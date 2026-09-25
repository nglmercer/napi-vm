use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::ptr::NonNull;
use std::rc::Rc;
#[cfg(target_has_atomic = "8")]
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(all(
    target_has_atomic = "8",
    target_has_atomic = "16",
    target_has_atomic = "32",
    target_has_atomic = "64"
))]
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64};

use crate::error::VmErr;
use crate::interpreter::{Env, Interpreter};
use crate::parser::Statement;

/// Hard cap on array length. Guest code that grows an array past this gets a
/// catchable `RangeError` instead of exhausting host memory (which would
/// abort the process — Rust's allocator does not return errors, it dies).
/// `Value` is 32 bytes, and arrays of arrays multiply that: 262k slots of
/// 8-element inner arrays is already ~290MB, so the cap is sized to keep
/// worst-case guest allocations survivable for the host.
pub const MAX_ARRAY_LEN: usize = 262_144;

/// Hard cap on the number of own properties in a guest object. Object
/// assignment is another unbounded allocation path even when arrays and
/// strings are capped.
pub const MAX_OBJECT_PROPS: usize = 262_144;

/// Hard cap on guest-created bindings in the persistent user-global scope.
/// Built-ins live in a separate parent environment and do not consume this
/// quota; local function/catch frames are also intentionally unaffected.
pub const MAX_GLOBAL_BINDINGS: usize = MAX_OBJECT_PROPS;

/// Hard cap (bytes) on any string the VM produces — concatenation, `repeat`,
/// `join`, `replaceAll`, `JSON.stringify`. Same rationale as `MAX_ARRAY_LEN`.
pub const MAX_STRING_LEN: usize = 16 * 1024 * 1024;

/// Maximum prototype links followed by a property lookup. Prototype chains
/// are guest-controlled and must not be allowed to consume the native stack
/// or spend unbounded time resolving a missing property.
pub const MAX_PROTOTYPE_DEPTH: usize = 4096;

/// Convenience constructor for the guest-visible limit errors.
pub fn limit_err(msg: &str) -> VmErr {
    VmErr::Msg(format!("RangeError: {}", msg))
}

/// Parse a canonical ECMAScript array-index property name. Strings such as
/// `"01"` and the reserved `"4294967295"` key are ordinary named properties.
///
/// Allocation-free: the previous `parse` + `to_string` comparison heap-
/// allocated on every numeric key, and this runs on each array property
/// access.
pub fn array_index(key: &str) -> Option<usize> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 {
        return None;
    }
    // Canonical form has no leading zeros: "01" is an ordinary property.
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    let mut index: u64 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        index = index * 10 + u64::from(b - b'0');
        // "4294967295" (`u32::MAX`) and anything larger are ordinary
        // properties; bailing here also makes overlong digit runs overflow-
        // free without parsing them fully.
        if index >= u32::MAX as u64 {
            return None;
        }
    }
    Some(index as usize)
}

/// Character length of a string as a guest `length` value. A byte scan is an
/// order of magnitude cheaper than UTF-8 decoding; only non-ASCII text pays
/// for `chars().count()`.
pub fn str_char_len(s: &str) -> f64 {
    if s.is_ascii() {
        s.len() as f64
    } else {
        s.chars().count() as f64
    }
}

/// The single-character string at char index `idx`, if any. When every byte
/// up to `idx` is ASCII the char index equals the byte index, so short
/// indices into long ASCII strings cost O(idx) instead of a full decode.
pub fn str_char_at(s: &str, idx: usize) -> Option<Value> {
    let bytes = s.as_bytes();
    if idx < bytes.len() && bytes[..=idx].is_ascii() {
        return Some(Value::String(s[idx..idx + 1].to_owned()));
    }
    s.chars().nth(idx).map(|c| Value::String(c.to_string()))
}

/// Per-property attributes (`writable`, `enumerable`, `configurable`).
///
/// Properties created by ordinary assignment or an object literal carry the
/// default `true`/`true`/`true` and are *not* stored: only properties whose
/// attributes differ from the default take a slot in [`ObjectMeta::attrs`], so
/// the common case costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropAttrs {
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

/// Runtime identity for built-in constructor objects whose instances use a
/// dedicated `Value` representation instead of an ordinary prototype link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinConstructor {
    Date,
}

/// Primitive payload retained by an ECMAScript wrapper object created through
/// `Object(value)` or Node-API's `napi_coerce_to_object`.
#[derive(Debug, Clone)]
pub enum BoxedPrimitive {
    Bool(bool),
    Number(f64),
    String(String),
    Symbol(Rc<SymbolData>),
    BigInt(Rc<crate::bigint::BigInt>),
}

impl Default for PropAttrs {
    fn default() -> Self {
        Self {
            writable: true,
            enumerable: true,
            configurable: true,
        }
    }
}

/// Object state that is *not* the property slots themselves: the prototype
/// link, per-property attributes, and extensibility.
///
/// This lives beside the slots inside one [`ObjectCell`] allocation, so every
/// clone of a `Value::Object` observes the same metadata. That sharing is what
/// makes `Object.setPrototypeOf`, `Object.freeze` and `defineProperty`
/// observable through every reference to the object rather than through the
/// one binding they were applied to.
#[derive(Debug, Default)]
pub struct ObjectMeta {
    /// Prototype link. `None` means either a null prototype or the runtime's
    /// default prototype, distinguished by `uses_default_prototype`.
    pub proto: Option<Rc<Value>>,
    /// Whether `proto == None` represents the runtime's default prototype.
    /// This distinguishes an implicit built-in link from an explicit null
    /// prototype for ordinary objects, functions, and arrays.
    pub uses_default_prototype: bool,
    /// Non-default property attributes, keyed by property name.
    pub attrs: Vec<(String, PropAttrs)>,
    /// Original identities for symbol-keyed property slots. The slot key keeps
    /// prototype lookup compact; this table preserves symbol descriptions and
    /// prevents a bridge from having to reconstruct a symbol from its id.
    pub symbol_keys: Vec<(String, Rc<SymbolData>)>,
    /// Cleared by `Object.preventExtensions`/`seal`/`freeze`: no new own
    /// properties may be added.
    pub non_extensible: bool,
    /// Whether `defineProperty` ever installed a getter/setter pair on this
    /// object. Ordinary objects never do, and property assignment checks this
    /// before looking for the companion slot an accessor pair needs — which
    /// is the difference between allocating a slot name on every write and
    /// never allocating one.
    pub has_accessors: bool,
    /// The primitive carried by a boxed Boolean, Number, String, Symbol, or
    /// BigInt object. Ordinary objects leave this empty.
    pub boxed_primitive: Option<BoxedPrimitive>,
    /// Some built-in instances (currently Date) have dedicated VM value
    /// variants, so their constructor identity cannot be recovered by walking
    /// an ordinary `[[Prototype]]` chain.
    pub(crate) builtin_constructor: Option<BuiltinConstructor>,
    /// Host bridge identity for a `Value::HostFunction`. Kept in the shared
    /// property cell so that value remains compact.
    pub(crate) host_function_id: Option<usize>,
}

impl ObjectMeta {
    pub fn attrs_of(&self, key: &str) -> PropAttrs {
        // Most objects never have a non-default attribute, and this runs on
        // every property read and write.
        if self.attrs.is_empty() {
            return PropAttrs::default();
        }
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, a)| *a)
            .unwrap_or_default()
    }

    pub fn set_attrs(&mut self, key: &str, attrs: PropAttrs) {
        if let Some((_, slot)) = self.attrs.iter_mut().find(|(k, _)| k == key) {
            *slot = attrs;
            return;
        }
        if attrs != PropAttrs::default() {
            self.attrs.push((key.to_string(), attrs));
        }
    }

    pub fn symbol_key(&self, key: &str) -> Option<Rc<SymbolData>> {
        self.symbol_keys
            .iter()
            .find(|(slot, _)| slot == key)
            .map(|(_, symbol)| symbol.clone())
    }

    pub fn set_symbol_key(&mut self, key: &str, symbol: Rc<SymbolData>) {
        if let Some((_, existing)) = self.symbol_keys.iter_mut().find(|(slot, _)| slot == key) {
            *existing = symbol;
        } else {
            self.symbol_keys.push((key.to_string(), symbol));
        }
    }

    pub fn forget(&mut self, key: &str) {
        self.attrs.retain(|(k, _)| k != key);
        self.symbol_keys.retain(|(k, _)| k != key);
    }
}

/// The allocation behind every `Value::Array`: the elements plus the named
/// properties an array can also carry.
///
/// Named properties are rare — a tagged template's `strings.raw` is the main
/// one — so the map is empty for ordinary arrays and costs a `Vec` header.
/// Like [`ObjectCell`], this `Deref`s to the element `RefCell` so existing
/// element access reads unchanged.
#[derive(Debug)]
pub struct ArrayCell {
    elements: RefCell<Vec<Value>>,
    /// Array indices can exist without owning a value. Element reads still
    /// return `undefined` for those indices; this bitmap preserves the
    /// observable distinction for `in`, `Object.hasOwn`, reflection, and host
    /// bridges.
    present: RefCell<Option<Vec<bool>>>,
    pub named: RefCell<Vec<(String, Value)>>,
    /// Prototype and property descriptors for named array properties. Array
    /// indices and `length` have their own storage and descriptor rules.
    pub meta: RefCell<ObjectMeta>,
    /// Original identities for symbol-keyed entries in `named`.
    pub symbol_keys: RefCell<Vec<(String, Rc<SymbolData>)>>,
}

fn array_meta() -> ObjectMeta {
    let mut meta = ObjectMeta {
        uses_default_prototype: true,
        ..ObjectMeta::default()
    };
    meta.set_attrs(
        "length",
        PropAttrs {
            writable: true,
            enumerable: false,
            configurable: false,
        },
    );
    meta
}

impl ArrayCell {
    pub fn new(elements: Vec<Value>) -> Self {
        Self {
            elements: RefCell::new(elements),
            present: RefCell::new(None),
            named: RefCell::new(Vec::new()),
            meta: RefCell::new(array_meta()),
            symbol_keys: RefCell::new(Vec::new()),
        }
    }

    pub fn with_presence(elements: Vec<Value>, present: Vec<bool>) -> Self {
        let mut normalized = present;
        normalized.resize(elements.len(), false);
        normalized.truncate(elements.len());
        let normalized = (!normalized.iter().all(|present| *present)).then_some(normalized);
        Self {
            elements: RefCell::new(elements),
            present: RefCell::new(normalized),
            named: RefCell::new(Vec::new()),
            meta: RefCell::new(array_meta()),
            symbol_keys: RefCell::new(Vec::new()),
        }
    }

    /// Apply an integrity level to array indices, named properties, and the
    /// special `length` property using the same descriptor metadata consumed
    /// by guest writes and Node-API property reflection.
    pub fn set_integrity(&self, freeze: bool) {
        let mut keys = self
            .presence_snapshot()
            .into_iter()
            .enumerate()
            .filter(|(_, present)| *present)
            .map(|(index, _)| index.to_string())
            .collect::<Vec<_>>();
        keys.extend(self.named.borrow().iter().map(|(key, _)| key.clone()));

        let mut meta = self.meta.borrow_mut();
        meta.non_extensible = true;
        let mut length = meta.attrs_of("length");
        length.enumerable = false;
        length.configurable = false;
        if freeze {
            length.writable = false;
        }
        meta.set_attrs("length", length);
        for key in keys {
            let mut attributes = meta.attrs_of(&key);
            attributes.configurable = false;
            if freeze {
                attributes.writable = false;
            }
            meta.set_attrs(&key, attributes);
        }
    }

    /// Set an array's length while respecting its own indexed property
    /// descriptors. Shrinking stops at the highest non-configurable index,
    /// matching the partial truncation performed by ArraySetLength.
    pub fn set_length(&self, requested: usize) {
        let old_length = self.elements.borrow().len();
        let mut length = requested;
        if requested < old_length {
            let meta = self.meta.borrow();
            for index in (requested..old_length).rev() {
                if self.has_index(index) && !meta.attrs_of(&index.to_string()).configurable {
                    length = index + 1;
                    break;
                }
            }
        }
        self.elements.borrow_mut().resize(length, Value::Undefined);
        self.resize_presence(old_length, length, false);
    }

    /// Whether every own array property satisfies the requested integrity
    /// level. Array length is non-configurable but remains writable when only
    /// sealed, as required by ECMAScript.
    pub fn is_integrity_locked(&self, freeze: bool) -> bool {
        let meta = self.meta.borrow();
        if !meta.non_extensible {
            return false;
        }
        let length = meta.attrs_of("length");
        if length.configurable || (freeze && length.writable) {
            return false;
        }
        let indices_locked = self
            .presence_snapshot()
            .into_iter()
            .enumerate()
            .filter(|(_, present)| *present)
            .all(|(index, _)| {
                let attributes = meta.attrs_of(&index.to_string());
                !attributes.configurable && (!freeze || !attributes.writable)
            });
        indices_locked
            && self.named.borrow().iter().all(|(key, _)| {
                let attributes = meta.attrs_of(key);
                !attributes.configurable && (!freeze || !attributes.writable)
            })
    }

    pub fn has_index(&self, index: usize) -> bool {
        let elements = self.elements.borrow();
        if index >= elements.len() {
            return false;
        }
        self.present
            .borrow()
            .as_ref()
            .and_then(|present| present.get(index))
            .copied()
            .unwrap_or(true)
    }

    pub fn set_index_presence(&self, index: usize, present: bool) {
        let length = self.elements.borrow().len();
        let mut presence = self.present.borrow_mut();
        if present && presence.is_none() {
            return;
        }
        let indices = presence.get_or_insert_with(|| vec![true; length]);
        indices.resize(length, true);
        if let Some(slot) = indices.get_mut(index) {
            *slot = present;
        }
        if indices.iter().all(|present| *present) {
            *presence = None;
        }
        drop(presence);
        if !present {
            self.meta.borrow_mut().forget(&index.to_string());
        }
    }

    pub fn presence_snapshot(&self) -> Vec<bool> {
        let length = self.elements.borrow().len();
        let presence = self.present.borrow();
        let Some(presence) = presence.as_ref() else {
            return vec![true; length];
        };
        let mut snapshot = presence.clone();
        snapshot.resize(length, true);
        snapshot.truncate(length);
        snapshot
    }

    pub fn replace_presence(&self, present: Vec<bool>) {
        let length = self.elements.borrow().len();
        let mut normalized = present;
        normalized.resize(length, false);
        normalized.truncate(length);
        *self.present.borrow_mut() =
            (!normalized.iter().all(|present| *present)).then_some(normalized);
    }

    pub fn resize_presence(&self, old_length: usize, length: usize, fill_present: bool) {
        let mut presence = self.present.borrow_mut();
        if let Some(present) = presence.as_mut() {
            present.resize(length, fill_present);
            present.truncate(length);
            if present.iter().all(|present| *present) {
                *presence = None;
            }
        } else if length < old_length {
            // A dense bitmap is unnecessary when truncating an all-present
            // array.
        } else if length > old_length && !fill_present {
            let mut present = vec![true; old_length];
            present.resize(length, false);
            *presence = Some(present);
        }
        drop(presence);
        if length < old_length {
            let mut metadata = self.meta.borrow_mut();
            for index in length..old_length {
                metadata.forget(&index.to_string());
            }
        }
    }

    pub fn append_present(&self, count: usize) {
        if let Some(present) = self.present.borrow_mut().as_mut() {
            present.resize(present.len().saturating_add(count), true);
        }
    }

    pub fn truncate_presence(&self, length: usize) {
        let mut presence = self.present.borrow_mut();
        if let Some(present) = presence.as_mut() {
            present.truncate(length);
            if present.iter().all(|present| *present) {
                *presence = None;
            }
        }
    }

    pub fn insert_present(&self, index: usize, count: usize) {
        if let Some(present) = self.present.borrow_mut().as_mut() {
            for offset in 0..count {
                present.insert(index.saturating_add(offset).min(present.len()), true);
            }
        }
    }

    pub fn remove_presence(&self, index: usize) {
        let mut presence = self.present.borrow_mut();
        if let Some(present) = presence.as_mut()
            && index < present.len()
        {
            present.remove(index);
            if present.iter().all(|present| *present) {
                *presence = None;
            }
        }
    }

    pub fn reverse_presence(&self) {
        if let Some(present) = self.present.borrow_mut().as_mut() {
            present.reverse();
        }
    }

    pub fn fill_presence(&self, start: usize, end: usize) {
        let mut presence = self.present.borrow_mut();
        if let Some(present) = presence.as_mut() {
            for slot in present.iter_mut().take(end).skip(start) {
                *slot = true;
            }
            if present.iter().all(|present| *present) {
                *presence = None;
            }
        }
    }

    pub fn named_prop(&self, key: &str) -> Option<Value> {
        self.named
            .borrow()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    pub fn proto(&self) -> Option<Rc<Value>> {
        self.meta.borrow().proto.clone()
    }

    pub fn set_proto(&self, proto: Option<Rc<Value>>) {
        let mut meta = self.meta.borrow_mut();
        meta.proto = proto;
        meta.uses_default_prototype = false;
    }

    pub fn set_named(&self, key: String, value: Value) {
        let mut named = self.named.borrow_mut();
        match named.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = value,
            None => named.push((key, value)),
        }
    }

    pub fn symbol_key(&self, key: &str) -> Option<Rc<SymbolData>> {
        self.symbol_keys
            .borrow()
            .iter()
            .find(|(slot, _)| slot == key)
            .map(|(_, symbol)| symbol.clone())
    }

    pub fn set_symbol_key(&self, key: &str, symbol: Rc<SymbolData>) {
        let mut keys = self.symbol_keys.borrow_mut();
        if let Some((_, existing)) = keys.iter_mut().find(|(slot, _)| slot == key) {
            *existing = symbol;
        } else {
            keys.push((key.to_owned(), symbol));
        }
    }

    pub fn forget_symbol_key(&self, key: &str) {
        self.symbol_keys
            .borrow_mut()
            .retain(|(slot, _)| slot != key);
    }

    /// Uncontended access to the elements, for the iterative `Drop`.
    pub fn elements_mut(&mut self) -> &mut Vec<Value> {
        self.elements.get_mut()
    }

    /// Child values for the cycle collector's marker: elements, named
    /// properties, and the prototype link.
    pub(crate) fn trace_children(&self) -> Vec<Value> {
        let mut out = Vec::new();
        if let Ok(elements) = self.elements.try_borrow() {
            out.extend(elements.iter().cloned());
        }
        if let Ok(named) = self.named.try_borrow() {
            out.extend(named.iter().map(|(_, v)| v.clone()));
        }
        if let Ok(meta) = self.meta.try_borrow() {
            out.extend(meta.proto.as_deref().cloned());
        }
        out
    }

    /// Drop this array's outgoing edges so an unreachable cycle can free.
    /// Only the collector calls this, and only for unmarked objects.
    pub(crate) fn clear_edges(&self) -> bool {
        let (Ok(mut elements), Ok(mut named), Ok(mut meta)) = (
            self.elements.try_borrow_mut(),
            self.named.try_borrow_mut(),
            self.meta.try_borrow_mut(),
        ) else {
            return false;
        };
        elements.clear();
        named.clear();
        meta.proto = None;
        true
    }
}

impl std::ops::Deref for ArrayCell {
    type Target = RefCell<Vec<Value>>;
    fn deref(&self) -> &Self::Target {
        &self.elements
    }
}

/// The single allocation behind every `Value::Object`: the property slots plus
/// the shared [`ObjectMeta`].
///
/// It `Deref`s to the slot `RefCell` so that `props.borrow()`,
/// `Rc::ptr_eq(props, other)` and the rest of the existing property-access
/// code keep working unchanged — the metadata is an addition beside the slots,
/// not a new indirection in front of them.
#[derive(Debug)]
pub struct ObjectCell {
    slots: RefCell<Vec<(String, Value)>>,
    pub meta: RefCell<ObjectMeta>,
    /// Cached canonical layout of the slot keys, built lazily on the
    /// second indexed read (see `own_index`). A cache, never authority:
    /// every indexed read verifies the key at the slot, and any mismatch
    /// rebuilds from the slots — so mutations that bypass the maintaining
    /// methods (host bridges writing through the `Deref`) only cost a
    /// rebuild.
    shape: RefCell<Option<Rc<crate::shape::Shape>>>,
    /// An indexed scan already ran on this object. Single-read objects —
    /// serialization, conversion, one-shot lookups — scan and stop here,
    /// never allocating a shape; only a repeat read builds the layout.
    read_once: Cell<bool>,
}

impl ObjectCell {
    pub fn new(props: Vec<(String, Value)>, proto: Option<Rc<Value>>) -> Self {
        Self {
            slots: RefCell::new(props),
            meta: RefCell::new(ObjectMeta {
                proto,
                ..ObjectMeta::default()
            }),
            shape: RefCell::new(None),
            read_once: Cell::new(false),
        }
    }

    pub fn new_with_default_proto(props: Vec<(String, Value)>) -> Self {
        Self {
            slots: RefCell::new(props),
            meta: RefCell::new(ObjectMeta {
                uses_default_prototype: true,
                ..ObjectMeta::default()
            }),
            shape: RefCell::new(None),
            read_once: Cell::new(false),
        }
    }

    pub fn proto(&self) -> Option<Rc<Value>> {
        self.meta.borrow().proto.clone()
    }

    pub fn set_proto(&self, proto: Option<Rc<Value>>) {
        let mut meta = self.meta.borrow_mut();
        meta.proto = proto;
        meta.uses_default_prototype = false;
    }

    /// Uncontended access to the slots, for the iterative `Drop`.
    pub fn slots_mut(&mut self) -> &mut Vec<(String, Value)> {
        self.slots.get_mut()
    }

    /// Child values for the cycle collector's marker: slot values plus the
    /// prototype link. A borrow conflict yields nothing — the marker keeps
    /// the node either way.
    pub(crate) fn trace_children(&self) -> Vec<Value> {
        let mut out = Vec::new();
        if let Ok(slots) = self.slots.try_borrow() {
            out.extend(slots.iter().map(|(_, v)| v.clone()));
        }
        if let Ok(meta) = self.meta.try_borrow() {
            out.extend(meta.proto.as_deref().cloned());
        }
        out
    }

    /// Drop this object's outgoing edges so an unreachable cycle can free.
    /// Only the collector calls this, and only for unmarked objects.
    pub(crate) fn clear_edges(&self) -> bool {
        let (Ok(mut slots), Ok(mut meta)) =
            (self.slots.try_borrow_mut(), self.meta.try_borrow_mut())
        else {
            return false;
        };
        slots.clear();
        meta.proto = None;
        // The layout is empty now; drop the cached shape so a later access
        // rebuilds instead of answering from a stale layout.
        if let Ok(mut shape) = self.shape.try_borrow_mut() {
            *shape = None;
        }
        self.read_once.set(false);
        true
    }

    /// Own slot index of `key`: the cached shape answers in O(1) when it
    /// agrees with the slots, and any disagreement — a key the shape does
    /// not know, an index whose key moved — falls back to the linear scan
    /// and rebuilds the shape from the slots. Absence the shape and the
    /// scan agree on keeps the shape untouched, so prototype-chain misses
    /// cost exactly what they always did.
    ///
    /// Shapes build on the *second* indexed read that finds its key. The
    /// first scan only marks the cell, so single-read objects never
    /// allocate a layout, and pure-miss traffic (prototype walks over
    /// foreign keys) never builds one either.
    pub(crate) fn own_index(&self, key: &str) -> Option<usize> {
        let slots = self.slots.borrow();
        let cached = self.shape.borrow().clone();
        if let Some(shape) = &cached
            && let Some(index) = shape.slot_of(key)
            && slots.get(index).is_some_and(|(k, _)| k == key)
        {
            return Some(index);
        }
        let found = slots.iter().position(|(k, _)| k == key);
        match &cached {
            None => {
                let second = self.read_once.replace(true);
                if second && found.is_some() {
                    *self.shape.borrow_mut() = Some(crate::shape::Shape::rebuild(
                        slots.iter().map(|(k, _)| k.as_str()),
                    ));
                }
            }
            Some(shape) => {
                // Agreed absence is the only keep: anything else means the
                // shape is stale or disagrees, and rebuilds.
                let agree = shape.slot_of(key).is_none() && found.is_none();
                if !agree {
                    *self.shape.borrow_mut() = Some(crate::shape::Shape::rebuild(
                        slots.iter().map(|(k, _)| k.as_str()),
                    ));
                }
            }
        }
        found
    }

    /// Clone the value at `index` after checking it still holds `key`.
    /// Inline-cache hits land here: the guard already matched the shape id,
    /// and this check is what keeps a stale shape from misreading.
    pub(crate) fn slot_verified(&self, index: usize, key: &str) -> Option<Value> {
        let slots = self.slots.borrow();
        let (k, v) = slots.get(index)?;
        (k == key).then(|| v.clone())
    }

    /// Clone an own property's raw slot value (bindings unresolved), if
    /// present. The indexed equivalent of the linear search it replaces.
    pub(crate) fn own_value(&self, key: &str) -> Option<Value> {
        let index = self.own_index(key)?;
        self.slot_verified(index, key)
    }

    /// This object's shape id, if a layout is cached. Never builds:
    /// unbuilt objects answer `None` (inline caches miss fast, guards
    /// fail) and only start caching once repeated reads build the shape.
    /// The id may lag a bypass mutation; every consumer verifies the key
    /// at the slot before trusting an id-indexed answer.
    pub(crate) fn shape_id(&self) -> Option<u32> {
        self.shape.borrow().as_ref().map(|shape| shape.id)
    }

    /// Whether a layout is cached. Writes consult this to skip indexed
    /// lookup entirely on cold objects.
    pub(crate) fn has_shape(&self) -> bool {
        self.shape.borrow().is_some()
    }

    /// Record a genuinely new key pushed onto the slots: follow the memoized
    /// transition so identically-built objects keep sharing one shape. A key
    /// the shape already knows means the push surprised the cache (or the
    /// shape lagged a bypass), so rebuild instead of forking a duplicate.
    pub(crate) fn note_key_added(&self, key: &str) {
        let cached = self.shape.borrow().clone();
        let Some(shape) = cached else {
            // Unbuilt shapes build lazily with the key already in place.
            return;
        };
        if shape.slot_of(key).is_some() {
            let slots = self.slots.borrow();
            *self.shape.borrow_mut() = Some(crate::shape::Shape::rebuild(
                slots.iter().map(|(k, _)| k.as_str()),
            ));
        } else {
            *self.shape.borrow_mut() = Some(shape.add(key));
        }
    }

    /// Forget the cached layout after a bulk mutation (delete, redefine,
    /// wholesale host-side replace). The next indexed access rebuilds from
    /// the slots; until then the object behaves exactly as before shapes.
    pub(crate) fn note_mutated(&self) {
        *self.shape.borrow_mut() = None;
    }
}

impl std::ops::Deref for ObjectCell {
    type Target = RefCell<Vec<(String, Value)>>;
    fn deref(&self) -> &Self::Target {
        &self.slots
    }
}

fn set_cell_prop(props: &ObjectCell, key: String, val: Value) -> Result<(), VmErr> {
    let writable = props.meta.borrow().attrs_of(&key).writable;
    // Cold objects scan linearly without touching the shape machinery:
    // writes never build layouts, only repeated reads do.
    let index = if props.has_shape() {
        props.own_index(&key)
    } else {
        props.borrow().iter().position(|(name, _)| *name == key)
    };
    if let Some(index) = index {
        if writable {
            props.borrow_mut()[index].1 = val;
        }
        return Ok(());
    }
    if props.meta.borrow().non_extensible {
        return Ok(());
    }
    let mut slots = props.borrow_mut();
    if slots.len() >= MAX_OBJECT_PROPS {
        return Err(limit_err("Maximum object property count exceeded"));
    }
    if props.has_shape() {
        slots.push((key.clone(), val));
        drop(slots);
        props.note_key_added(&key);
    } else {
        slots.push((key, val));
    }
    Ok(())
}

/// A symbol's identity.
///
/// A symbol is unique: two symbols with the same description are different
/// values, and `s === s` is true only for the same one. That is why the
/// description alone cannot represent it — identity lives in `id`, which is
/// what `strict_equals` and the property-slot naming both compare.
///
/// Ids below [`FIRST_USER_SYMBOL`] are reserved for the well-known symbols, so
/// `Symbol.iterator` is the same value every time it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolData {
    pub id: u64,
    /// `Symbol()` has no description; `Symbol('x')` has `"x"`.
    pub description: Option<String>,
}

/// Ids `0..FIRST_USER_SYMBOL` name the well-known symbols.
pub const FIRST_USER_SYMBOL: u64 = 64;

impl SymbolData {
    /// `String(sym)` / `sym.toString()`: `Symbol(desc)`, or `Symbol()`.
    pub fn to_display(&self) -> String {
        match &self.description {
            Some(d) => format!("Symbol({})", d),
            None => "Symbol()".to_string(),
        }
    }
}

/// Internal state for a bound callable.
#[derive(Debug, Clone)]
pub struct BoundFunctionData {
    pub target: Value,
    pub this_value: Value,
    pub arguments: Rc<Vec<Value>>,
}

/// Payload of `Value::Function`, boxed so the enum itself stays small.
#[derive(Debug, Clone)]
pub struct FunctionData {
    /// Shared identity for this function object. Cloning a VM `Value` keeps the
    /// same identity, while evaluating the same function expression again
    /// creates a distinct one.
    pub identity: Rc<u8>,
    pub name: Option<Rc<str>>,
    /// Shared own properties for this function object. A function's ordinary
    /// prototype object is created on first access so unused functions do not
    /// allocate prototype objects or form reference cycles.
    pub properties: Rc<ObjectCell>,
    /// Whether the standard own `name` and `length` descriptors have been
    /// materialized in `properties`. Shared with function clones so deleting
    /// one of those configurable properties does not recreate it later.
    pub standard_properties_initialized: Rc<Cell<bool>>,
    // Shared (`Rc`) so closures created in hot loops reference the same AST
    // instead of deep-cloning the parameter list and body on every creation.
    // Param names are `Rc<str>` so binding them in a call frame is a refcount
    // bump, not a heap allocation.
    pub params: Rc<Vec<Rc<str>>>,
    pub body: Rc<Vec<Statement>>,
    pub closure: Option<Env>,
    pub is_arrow: bool,
    /// Whether `new` may construct this function. Methods, accessors, arrows,
    /// async functions, and generator functions are not constructors.
    pub is_constructor: bool,
    pub is_async: bool,
    pub is_generator: bool,
    /// Whether the body references `arguments`. Frames for functions that
    /// never read it skip building the (detached) arguments object.
    pub uses_arguments: bool,
    /// Whether the body declares anything hoistable (`var`/`let`/`const`,
    /// function, or class). Computed once at creation so calls skip both
    /// hoist passes — a recursive walk plus a `Vec` allocation — when the
    /// body has nothing to hoist.
    pub needs_hoisting: bool,
    /// The bound target and arguments for functions created by
    /// `Function.prototype.bind`.
    pub bound: Option<Rc<BoundFunctionData>>,
    /// Compiled bytecode for this function, when the Phase E compiler
    /// accepted its body. `None` runs the AST `body`; `Some` runs the
    /// bytecode VM with identical semantics.
    pub bytecode: Option<Rc<crate::bytecode::BytecodeFunction>>,
}

impl FunctionData {
    /// Get this realm's intrinsic `Function.prototype`, when the built-ins
    /// have installed it. Function objects created during bootstrap use an
    /// explicit prototype link and do not call this helper recursively.
    pub fn default_function_prototype(global: &Env) -> Option<Value> {
        global
            .borrow()
            .get("Function")
            .and_then(|constructor| constructor.get_prop("prototype"))
    }

    /// Create own-property storage linked to the realm's Function.prototype.
    pub fn properties_with_default_prototype(global: &Env) -> Rc<ObjectCell> {
        let properties =
            crate::heap::tracked(Rc::new(ObjectCell::new_with_default_proto(Vec::new())));
        if let Some(prototype) = Self::default_function_prototype(global) {
            properties.set_proto(Some(Rc::new(prototype)));
        }
        properties
    }

    pub fn ensure_name_length_properties(&self) {
        if self.standard_properties_initialized.replace(true) {
            return;
        }
        let mut properties = self.properties.borrow_mut();
        if !properties.iter().any(|(key, _)| key == "length") {
            properties.push((
                "length".to_string(),
                Value::Number(
                    self.params
                        .iter()
                        .take_while(|parameter| !parameter.starts_with("..."))
                        .count() as f64,
                ),
            ));
            self.properties.meta.borrow_mut().set_attrs(
                "length",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        if !properties.iter().any(|(key, _)| key == "name") {
            properties.push((
                "name".to_string(),
                Value::String(self.name.as_deref().unwrap_or_default().to_string()),
            ));
            self.properties.meta.borrow_mut().set_attrs(
                "name",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        drop(properties);
        // Runs once per function: materializing `length`/`name` changes the
        // layout, and the next indexed access rebuilds the shape for it.
        self.properties.note_mutated();
    }

    /// Return the function's own `prototype` property, creating the standard
    /// object lazily for constructable ordinary functions.
    pub fn prototype_value(&self, function: &Value) -> Value {
        self.ensure_name_length_properties();
        if let Some(value) = self
            .properties
            .borrow()
            .iter()
            .find(|(key, _)| key == "prototype")
            .map(|(_, value)| value.deref_binding())
        {
            return value;
        }
        if self.bound.is_some() {
            return Value::Undefined;
        }
        if !self.is_constructor {
            return Value::Undefined;
        }

        let prototype = Value::object(vec![("constructor".to_string(), function.clone())]);
        if let Value::Object { props } = &prototype {
            props.meta.borrow_mut().set_attrs(
                "constructor",
                PropAttrs {
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let mut properties = self.properties.borrow_mut();
        // FunctionData is !Send and all guest execution is on one owner
        // thread, but recheck after allocation to keep this helper robust to
        // future host callbacks added during prototype creation.
        if let Some(value) = properties
            .iter()
            .find(|(key, _)| key == "prototype")
            .map(|(_, value)| value.deref_binding())
        {
            return value;
        }
        properties.push(("prototype".to_string(), prototype.clone()));
        self.properties.meta.borrow_mut().set_attrs(
            "prototype",
            PropAttrs {
                writable: true,
                enumerable: false,
                configurable: false,
            },
        );
        prototype
    }
}

/// Lazy state for a string iterator. The source is shared and the cursor is a
/// UTF-8 byte offset, so `next()` creates only the one scalar value requested.
#[derive(Debug, Clone)]
pub struct StringIteratorData {
    pub source: Rc<str>,
    pub cursor: usize,
}

/// Payload of `Value::Class`, boxed so the enum itself stays small.
#[derive(Debug, Clone)]
pub struct ClassData {
    pub name: String,
    pub constructor: Box<Value>,
    // Shared so every instance references the same prototype object (cheap
    // `Rc` clone, and identity-comparable for `instanceof`).
    pub prototype: Rc<Value>,
    /// Constructor-owned properties. Sharing the ordinary object cell gives
    /// class statics the same descriptors, symbols, and accessor storage as
    /// other JavaScript objects.
    pub statics: Rc<ObjectCell>,
}

/// Payload of `Value::RegExp`.
#[derive(Debug)]
pub struct RegExpData {
    pub regex: crate::regex::Regex,
    /// Where the next `g`/`y` search starts. Guest-writable.
    pub last_index: std::cell::Cell<usize>,
}

/// Backing storage for owned and native-owned ArrayBuffer byte ranges.
#[derive(Debug)]
enum BufferStorage {
    Owned(Vec<u8>),
    External { data: NonNull<u8>, length: usize },
    Detached,
}

impl BufferStorage {
    unsafe fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::External { data, length } => unsafe {
                std::slice::from_raw_parts(data.as_ptr(), *length)
            },
            Self::Detached => &[],
        }
    }

    unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::External { data, length } => unsafe {
                std::slice::from_raw_parts_mut(data.as_ptr(), *length)
            },
            Self::Detached => &mut [],
        }
    }
}

/// A byte store shared by an ArrayBuffer and its views. External stores point
/// at memory owned by a trusted native addon; that addon must keep it valid
/// until its Node-API finalizer runs and synchronize any concurrent access.
#[derive(Debug, Clone)]
pub struct Buffer(Rc<RefCell<BufferStorage>>);

impl Buffer {
    pub fn owned(bytes: Vec<u8>) -> Self {
        Self(Rc::new(RefCell::new(BufferStorage::Owned(bytes))))
    }

    pub fn zeroed(length: usize) -> Self {
        Self::owned(vec![0; length])
    }

    /// Wrap a native-owned byte range without copying it.
    ///
    /// # Safety
    /// `data` must point to `length` readable and writable bytes and remain
    /// alive until all guest views are unusable and the addon finalizer runs.
    pub unsafe fn external(data: *mut u8, length: usize) -> Option<Self> {
        let data = match NonNull::new(data) {
            Some(data) => data,
            None if length == 0 => NonNull::dangling(),
            None => return None,
        };
        Some(Self(Rc::new(RefCell::new(BufferStorage::External {
            data,
            length,
        }))))
    }

    pub fn borrow(&self) -> Ref<'_, [u8]> {
        Ref::map(self.0.borrow(), |storage| unsafe { storage.as_slice() })
    }

    pub fn borrow_mut(&self) -> RefMut<'_, [u8]> {
        RefMut::map(self.0.borrow_mut(), |storage| unsafe {
            storage.as_mut_slice()
        })
    }

    pub fn identity(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }

    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    pub(crate) fn strong_count(&self) -> usize {
        Rc::strong_count(&self.0)
    }

    pub fn is_detached(&self) -> bool {
        matches!(*self.0.borrow(), BufferStorage::Detached)
    }

    /// Detach a buffer, invalidating the backing store shared by all views.
    /// Repeated detachment is idempotent, matching Node's current Node-API
    /// behavior for both VM-owned and externally backed ArrayBuffers.
    pub fn detach(&self) {
        let mut storage = self.0.borrow_mut();
        if !matches!(*storage, BufferStorage::Detached) {
            *storage = BufferStorage::Detached;
        }
    }
}

/// Backing storage for a `SharedArrayBuffer`. Owned bytes are allocated with
/// alignment suitable for every Atomics element width and accessed through
/// byte atomics so native workers cannot race ordinary Rust slice access.
/// External storage is owned by a trusted addon and follows the same lifetime
/// and synchronization contract as external ArrayBuffers.
#[derive(Debug)]
enum SharedByteStorage {
    Owned {
        data: NonNull<u8>,
        length: usize,
        layout: Layout,
    },
    External {
        data: NonNull<u8>,
        length: usize,
    },
}

unsafe fn load_shared_byte(pointer: *mut u8) -> u8 {
    #[cfg(target_has_atomic = "8")]
    {
        // SAFETY: callers validate the pointer's byte range. AtomicU8 has
        // byte alignment and its representation matches a byte.
        unsafe { &*pointer.cast::<AtomicU8>() }.load(Ordering::SeqCst)
    }
    #[cfg(not(target_has_atomic = "8"))]
    {
        // Targets without byte atomics cannot share this memory with native
        // workers; this fallback keeps the value model available there.
        unsafe { pointer.read() }
    }
}

unsafe fn store_shared_byte(pointer: *mut u8, value: u8) {
    #[cfg(target_has_atomic = "8")]
    {
        // SAFETY: callers validate the pointer's byte range. AtomicU8 has
        // byte alignment and its representation matches a byte.
        unsafe { &*pointer.cast::<AtomicU8>() }.store(value, Ordering::SeqCst);
    }
    #[cfg(not(target_has_atomic = "8"))]
    {
        unsafe { pointer.write(value) };
    }
}

impl SharedByteStorage {
    fn data(&self) -> NonNull<u8> {
        match self {
            Self::Owned { data, .. } | Self::External { data, .. } => *data,
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Owned { length, .. } | Self::External { length, .. } => *length,
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        (0..self.len())
            .map(|index| {
                // SAFETY: the allocation covers `len` bytes and AtomicU8 has
                // byte alignment. The caller must use atomic access whenever
                // native code can access this shared memory concurrently.
                unsafe { load_shared_byte(self.data().as_ptr().add(index)) }
            })
            .collect()
    }

    fn read(&self, offset: usize, length: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(length)?;
        if end > self.len() {
            return None;
        }
        Some(
            (offset..end)
                .map(|index| {
                    // SAFETY: the range is within the backing store and
                    // AtomicU8 permits access at every byte address.
                    unsafe { load_shared_byte(self.data().as_ptr().add(index)) }
                })
                .collect(),
        )
    }

    fn write(&self, offset: usize, bytes: &[u8]) -> bool {
        let Some(end) = offset.checked_add(bytes.len()) else {
            return false;
        };
        if end > self.len() {
            return false;
        }
        for (index, byte) in bytes.iter().copied().enumerate() {
            // SAFETY: bounds are checked above, the pointer is byte-aligned,
            // and all VM accesses to shared storage use AtomicU8.
            unsafe { store_shared_byte(self.data().as_ptr().add(offset + index), byte) };
        }
        true
    }
}

impl Drop for SharedByteStorage {
    fn drop(&mut self) {
        if let Self::Owned { data, layout, .. } = self {
            // SAFETY: this pointer and layout are the exact pair returned by
            // `alloc_zeroed` in `SharedBuffer::zeroed`.
            unsafe { dealloc(data.as_ptr(), *layout) };
        }
    }
}

/// A guest `SharedArrayBuffer`. Cloning this handle preserves JS object
/// identity. Its byte store is separate from ordinary `ArrayBuffer` storage.
#[derive(Debug)]
struct SharedArrayBufferData {
    bytes: Rc<SharedByteStorage>,
}

#[derive(Debug, Clone)]
pub struct SharedBuffer(Rc<SharedArrayBufferData>);

#[derive(Debug, Clone, Copy)]
pub enum SharedAtomicOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Exchange,
    CompareExchange,
}

impl SharedBuffer {
    pub fn zeroed(length: usize) -> Option<Self> {
        // Pad the allocation so Atomics can address aligned 16/32/64-bit
        // elements through the pointer returned by Node-API.
        let allocation_length = length.max(1).checked_add(7)? & !7;
        let layout = Layout::from_size_align(allocation_length, 8).ok()?;
        // SAFETY: layout is non-zero and valid; the storage is deallocated by
        // SharedByteStorage::drop.
        let data = NonNull::new(unsafe { alloc_zeroed(layout) })?;
        Some(Self(Rc::new(SharedArrayBufferData {
            bytes: Rc::new(SharedByteStorage::Owned {
                data,
                length,
                layout,
            }),
        })))
    }

    /// Wrap addon-owned bytes as a shared buffer without copying them.
    ///
    /// # Safety
    /// `data` must point to `length` readable and writable bytes and remain
    /// alive until the registered native finalizer runs. Native threads that
    /// access these bytes concurrently with the VM must use compatible atomic
    /// operations and synchronization.
    pub unsafe fn external(data: *mut u8, length: usize) -> Option<Self> {
        let data = match NonNull::new(data) {
            Some(data) => data,
            None if length == 0 => NonNull::dangling(),
            None => return None,
        };
        Some(Self(Rc::new(SharedArrayBufferData {
            bytes: Rc::new(SharedByteStorage::External { data, length }),
        })))
    }

    pub fn len(&self) -> usize {
        self.0.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.0.bytes.snapshot()
    }

    pub fn read(&self, offset: usize, length: usize) -> Option<Vec<u8>> {
        self.0.bytes.read(offset, length)
    }

    pub fn write(&self, offset: usize, bytes: &[u8]) -> bool {
        self.0.bytes.write(offset, bytes)
    }

    pub fn data_ptr(&self) -> *mut u8 {
        self.0.bytes.data().as_ptr()
    }

    pub fn identity(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }

    /// Identity of the shared data block, which remains the same when a
    /// SharedArrayBuffer object is structured-cloned into another wrapper.
    pub fn wait_identity(&self) -> usize {
        Rc::as_ptr(&self.0.bytes) as usize
    }

    fn atomic_pointer(&self, offset: usize, width: usize) -> Option<*mut u8> {
        if !matches!(width, 1 | 2 | 4 | 8)
            || !offset.is_multiple_of(width)
            || offset.checked_add(width)? > self.len()
        {
            return None;
        }
        let pointer = unsafe { self.data_ptr().add(offset) };
        ((pointer as usize).is_multiple_of(width)).then_some(pointer)
    }

    #[cfg(all(
        target_has_atomic = "8",
        target_has_atomic = "16",
        target_has_atomic = "32",
        target_has_atomic = "64"
    ))]
    pub fn atomic_load(&self, offset: usize, width: usize) -> Option<u64> {
        let pointer = self.atomic_pointer(offset, width)?;
        // SAFETY: `atomic_pointer` checks bounds and alignment. The shared
        // allocation is initialized to zero and VM/native concurrent access
        // is required to use atomic operations.
        Some(unsafe {
            match width {
                1 => (&*pointer.cast::<AtomicU8>()).load(Ordering::SeqCst) as u64,
                2 => (&*pointer.cast::<AtomicU16>()).load(Ordering::SeqCst) as u64,
                4 => (&*pointer.cast::<AtomicU32>()).load(Ordering::SeqCst) as u64,
                8 => (&*pointer.cast::<AtomicU64>()).load(Ordering::SeqCst),
                _ => return None,
            }
        })
    }

    #[cfg(not(all(
        target_has_atomic = "8",
        target_has_atomic = "16",
        target_has_atomic = "32",
        target_has_atomic = "64"
    )))]
    pub fn atomic_load(&self, _offset: usize, _width: usize) -> Option<u64> {
        None
    }

    #[cfg(all(
        target_has_atomic = "8",
        target_has_atomic = "16",
        target_has_atomic = "32",
        target_has_atomic = "64"
    ))]
    pub fn atomic_store(&self, offset: usize, width: usize, value: u64) -> bool {
        let Some(pointer) = self.atomic_pointer(offset, width) else {
            return false;
        };
        // SAFETY: `atomic_pointer` checks bounds and alignment. All concurrent
        // VM/native accesses to this shared allocation are atomic.
        unsafe {
            match width {
                1 => (&*pointer.cast::<AtomicU8>()).store(value as u8, Ordering::SeqCst),
                2 => (&*pointer.cast::<AtomicU16>()).store(value as u16, Ordering::SeqCst),
                4 => (&*pointer.cast::<AtomicU32>()).store(value as u32, Ordering::SeqCst),
                8 => (&*pointer.cast::<AtomicU64>()).store(value, Ordering::SeqCst),
                _ => return false,
            }
        }
        true
    }

    #[cfg(not(all(
        target_has_atomic = "8",
        target_has_atomic = "16",
        target_has_atomic = "32",
        target_has_atomic = "64"
    )))]
    pub fn atomic_store(&self, _offset: usize, _width: usize, _value: u64) -> bool {
        false
    }

    #[cfg(all(
        target_has_atomic = "8",
        target_has_atomic = "16",
        target_has_atomic = "32",
        target_has_atomic = "64"
    ))]
    pub fn atomic_rmw(
        &self,
        offset: usize,
        width: usize,
        operation: SharedAtomicOp,
        value: u64,
        replacement: u64,
    ) -> Option<u64> {
        let pointer = self.atomic_pointer(offset, width)?;
        // SAFETY: `atomic_pointer` checks bounds and alignment. All concurrent
        // VM/native accesses to this shared allocation are atomic.
        Some(unsafe {
            macro_rules! apply {
                ($atomic:ty, $value:expr, $replacement:expr) => {{
                    let atomic = &*pointer.cast::<$atomic>();
                    match operation {
                        SharedAtomicOp::Add => atomic.fetch_add($value, Ordering::SeqCst),
                        SharedAtomicOp::Sub => atomic.fetch_sub($value, Ordering::SeqCst),
                        SharedAtomicOp::And => atomic.fetch_and($value, Ordering::SeqCst),
                        SharedAtomicOp::Or => atomic.fetch_or($value, Ordering::SeqCst),
                        SharedAtomicOp::Xor => atomic.fetch_xor($value, Ordering::SeqCst),
                        SharedAtomicOp::Exchange => atomic.swap($value, Ordering::SeqCst),
                        SharedAtomicOp::CompareExchange => atomic
                            .compare_exchange(
                                $value,
                                $replacement,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            )
                            .unwrap_or_else(|observed| observed),
                    }
                }};
            }
            match width {
                1 => apply!(AtomicU8, value as u8, replacement as u8) as u64,
                2 => apply!(AtomicU16, value as u16, replacement as u16) as u64,
                4 => apply!(AtomicU32, value as u32, replacement as u32) as u64,
                8 => apply!(AtomicU64, value, replacement),
                _ => return None,
            }
        })
    }

    #[cfg(not(all(
        target_has_atomic = "8",
        target_has_atomic = "16",
        target_has_atomic = "32",
        target_has_atomic = "64"
    )))]
    pub fn atomic_rmw(
        &self,
        _offset: usize,
        _width: usize,
        _operation: SharedAtomicOp,
        _value: u64,
        _replacement: u64,
    ) -> Option<u64> {
        None
    }

    pub fn is_lock_free(width: usize) -> bool {
        matches!(
            (
                width,
                cfg!(target_has_atomic = "8"),
                cfg!(target_has_atomic = "16"),
                cfg!(target_has_atomic = "32"),
                cfg!(target_has_atomic = "64"),
            ),
            (1, true, _, _, _) | (2, _, true, _, _) | (4, _, _, true, _) | (8, _, _, _, true)
        )
    }

    /// Structured cloning creates a distinct SAB object over the same shared
    /// data block, as required by the structured clone algorithm.
    pub fn shared_clone(&self) -> Self {
        Self(Rc::new(SharedArrayBufferData {
            bytes: self.0.bytes.clone(),
        }))
    }
}

/// The kind of byte buffer underlying a typed array or DataView.
#[derive(Debug, Clone)]
pub enum BufferBacking {
    Array(Buffer),
    Shared(SharedBuffer),
}

impl From<Buffer> for BufferBacking {
    fn from(buffer: Buffer) -> Self {
        Self::Array(buffer)
    }
}

impl From<SharedBuffer> for BufferBacking {
    fn from(buffer: SharedBuffer) -> Self {
        Self::Shared(buffer)
    }
}

impl BufferBacking {
    pub fn len(&self) -> usize {
        match self {
            Self::Array(buffer) => buffer.borrow().len(),
            Self::Shared(buffer) => buffer.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_detached(&self) -> bool {
        match self {
            Self::Array(buffer) => buffer.is_detached(),
            Self::Shared(_) => false,
        }
    }

    pub fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }

    pub fn identity(&self) -> usize {
        match self {
            Self::Array(buffer) => buffer.identity(),
            Self::Shared(buffer) => buffer.identity(),
        }
    }

    pub fn snapshot(&self) -> Vec<u8> {
        match self {
            Self::Array(buffer) => buffer.borrow().to_vec(),
            Self::Shared(buffer) => buffer.snapshot(),
        }
    }

    pub fn read(&self, offset: usize, length: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(length)?;
        if end > self.len() {
            return None;
        }
        match self {
            Self::Array(buffer) => Some(buffer.borrow()[offset..end].to_vec()),
            Self::Shared(buffer) => buffer.read(offset, length),
        }
    }

    pub fn write(&self, offset: usize, bytes: &[u8]) -> bool {
        let end = match offset.checked_add(bytes.len()) {
            Some(end) if end <= self.len() => end,
            _ => return false,
        };
        match self {
            Self::Array(buffer) => {
                buffer.borrow_mut()[offset..end].copy_from_slice(bytes);
                true
            }
            Self::Shared(buffer) => buffer.write(offset, bytes),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Array(buffer) => Value::ArrayBuffer(buffer.clone()),
            Self::Shared(buffer) => Value::SharedArrayBuffer(buffer.clone()),
        }
    }

    pub fn data_ptr(&self) -> *mut u8 {
        match self {
            Self::Array(buffer) => {
                if buffer.is_detached() {
                    return std::ptr::null_mut();
                }
                buffer.borrow_mut().as_mut_ptr()
            }
            Self::Shared(buffer) => buffer.data_ptr(),
        }
    }
}

/// A typed array's element type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    Int8,
    Uint8,
    /// `Uint8ClampedArray`: saturates instead of wrapping, and rounds to
    /// nearest instead of truncating.
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl TypedKind {
    pub fn size(self) -> usize {
        match self {
            TypedKind::Int8 | TypedKind::Uint8 | TypedKind::Uint8Clamped => 1,
            TypedKind::Int16 | TypedKind::Uint16 => 2,
            TypedKind::Int32 | TypedKind::Uint32 | TypedKind::Float32 => 4,
            TypedKind::Float64 | TypedKind::BigInt64 | TypedKind::BigUint64 => 8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TypedKind::Int8 => "Int8Array",
            TypedKind::Uint8 => "Uint8Array",
            TypedKind::Uint8Clamped => "Uint8ClampedArray",
            TypedKind::Int16 => "Int16Array",
            TypedKind::Uint16 => "Uint16Array",
            TypedKind::Int32 => "Int32Array",
            TypedKind::Uint32 => "Uint32Array",
            TypedKind::Float32 => "Float32Array",
            TypedKind::Float64 => "Float64Array",
            TypedKind::BigInt64 => "BigInt64Array",
            TypedKind::BigUint64 => "BigUint64Array",
        }
    }
}

/// A window onto a buffer: shared by the typed arrays and `DataView`, which
/// differ only in how they interpret it.
#[derive(Debug)]
pub struct TypedArrayData {
    pub kind: TypedKind,
    pub buffer: BufferBacking,
    pub byte_offset: usize,
    /// Element count for a typed array; *byte* count for a `DataView`.
    pub length: usize,
    /// Node's `Buffer` subclasses `Uint8Array`, but keeps distinct prototype
    /// and coercion behavior. The shared storage shape represents both.
    pub is_buffer: bool,
}

impl TypedArrayData {
    pub fn effective_length(&self) -> usize {
        if self.buffer.is_detached() {
            0
        } else {
            self.length
        }
    }

    pub fn effective_byte_offset(&self) -> usize {
        if self.buffer.is_detached() {
            0
        } else {
            self.byte_offset
        }
    }
}

/// Payload of `Value::Proxy`.
#[derive(Debug)]
pub struct ProxyData {
    pub target: Value,
    pub handler: Value,
}

/// Payload of `Value::Error`, boxed so the enum itself stays small.
#[derive(Debug, Clone)]
pub struct ErrorData {
    /// Clones of a guest error value must retain object identity, while two
    /// separately-created errors with the same fields remain distinct.
    pub identity: Rc<()>,
    pub message: String,
    pub name: String,
    /// Optional runtime-specific error identifier such as Node's `code`.
    pub code: Option<String>,
    /// The call stack where the error was raised, rendered the way engines
    /// print it. Empty when there was no frame to record.
    pub stack: String,
}

impl ErrorData {
    /// An error with no recorded stack — the shape a host- or
    /// combinator-produced error takes, where there was no guest frame.
    pub fn new(name: &str, message: impl Into<String>) -> Box<Self> {
        Box::new(Self {
            identity: Rc::new(()),
            name: name.to_string(),
            message: message.into(),
            stack: String::new(),
            code: None,
        })
    }

    /// An error carrying a stable runtime or host error identifier.
    pub fn with_code(name: &str, message: impl Into<String>, code: impl Into<String>) -> Box<Self> {
        Box::new(Self {
            identity: Rc::new(()),
            message: message.into(),
            name: name.to_string(),
            stack: String::new(),
            code: Some(code.into()),
        })
    }
}

/// Every variant's inline payload is at most 24 bytes (a `String`), so the
/// whole enum is 32 bytes. Keeping `Value` small matters: it is returned from
/// every `eval_expr`/`eval_stmt`/`bin_op` call and cloned on every variable
/// read. Function payloads use `Rc` so reading a function binding does not
/// allocate a fresh payload on every call; class and error payloads are boxed.
#[derive(Debug, Clone)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Object {
        props: Rc<ObjectCell>,
    },
    Array(Rc<ArrayCell>),
    Function(Rc<FunctionData>),
    NativeFunction {
        name: Rc<str>,
        callable: fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>,
    },
    /// A function implemented on the host (Node.js) side, reachable from the VM.
    /// Calling it dispatches through the interpreter's `HostBridge` using `id`,
    /// which the bridge maps to a persisted JavaScript function reference.
    HostFunction {
        name: Rc<str>,
        /// Own properties of this host-backed function object. Node-API
        /// callbacks are ordinary JavaScript functions from the guest's
        /// point of view, so they need shared property descriptors and
        /// integrity metadata like interpreter-created functions.
        properties: Rc<ObjectCell>,
    },
    /// A handle to the global scope itself. Bound to `globalThis`, `self` and
    /// `window`; member access on it reads and writes real globals (handled in
    /// `Interpreter::prop` / `assign_member`, which have scope access).
    GlobalObject,
    Class(Box<ClassData>),
    Promise(Rc<RefCell<PromiseInner>>),
    Generator {
        inner: Rc<RefCell<GeneratorInner>>,
    },
    StringIterator {
        inner: Rc<RefCell<StringIteratorData>>,
    },
    /// Sentinel returned when an async host function is called. The interpreter
    /// recognizes this at `await` and parks the VM thread until the host
    /// resolves the pending operation via the async channel.
    HostPending {
        id: usize,
    },
    Symbol(Rc<SymbolData>),
    /// A `Date`: epoch milliseconds in a shared, mutable cell, so `setTime`
    /// is observed through every reference.
    Date(Rc<std::cell::Cell<f64>>),
    /// A `Proxy`: a target and the handler whose traps intercept operations
    /// on it. An operation the handler does not trap falls through.
    Proxy(Rc<ProxyData>),
    /// Raw bytes. Shared, so every view onto it sees the same storage.
    ArrayBuffer(Buffer),
    /// Shared raw bytes with atomic backing storage for native workers.
    SharedArrayBuffer(SharedBuffer),
    /// A typed view onto a buffer: an element type plus a window.
    TypedArray(Rc<TypedArrayData>),
    /// A `DataView`: the same window, read and written one element at a time
    /// with an explicit type and byte order.
    DataView(Rc<TypedArrayData>),
    /// An arbitrary-precision integer. A separate numeric type, not a wider
    /// `Number`: mixing the two in arithmetic is a `TypeError`, which is what
    /// keeps `BigInt` from silently losing precision.
    BigInt(Rc<crate::bigint::BigInt>),
    /// A compiled regular expression. `lastIndex` is mutable and shared with
    /// every reference, which is what makes a `/g/` pattern advance across
    /// successive `exec` calls.
    RegExp(Rc<RegExpData>),
    /// A suspended async call, carried through the reaction functions that
    /// resume it. Internal: it never reaches guest code.
    #[cfg(stackful_coroutines)]
    AsyncTask(Rc<RefCell<crate::interpreter::AsyncTask>>),
    /// A *live binding*: an indirection an ES module export and its importers
    /// share, so a write on either side is seen by the other.
    ///
    /// This never reaches guest code. Every path that reads a binding or a
    /// property resolves it first (see [`Value::deref_binding`]); it exists
    /// only in an environment slot, in a module's export table, and in a
    /// namespace object's slots.
    Binding(Rc<RefCell<Value>>),
    Error(Box<ErrorData>),
}

// Guard the hot-path size: every eval function returns `Value` (inside a
// `Result`) and every variable read clones one. If a future variant bloats
// the enum, this assert fails at compile time — box its payload instead.
const _: () = assert!(std::mem::size_of::<Value>() <= 32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

/// One `then` registration waiting for a promise to settle.
///
/// `derived` is the promise `then` handed back; settling it is what propagates
/// the handler's result (or the absence of a handler) down the chain.
#[derive(Debug)]
pub struct Reaction {
    pub on_fulfilled: Value,
    pub on_rejected: Value,
    pub derived: Rc<RefCell<PromiseInner>>,
    /// Adoption reactions settle their target directly after following the
    /// source promise; they must not re-enter the target's already-used
    /// resolve function.
    pub adopted: bool,
}

/// The shared state of one promise.
///
/// A promise is a mutable, *shared* object: `p.then(…)` on any reference must
/// see settlements caused through any other, which is why this lives behind an
/// `Rc<RefCell<…>>` rather than inline in the `Value`.
#[derive(Debug)]
pub struct PromiseInner {
    pub state: PromiseState,
    /// The promise's resolve or reject function has already been called.
    /// Resolution can remain pending while it adopts a thenable or promise.
    pub resolution_locked: bool,
    /// Set while settlement depends on an external host event. This marker is
    /// propagated to chained promises so a synchronous await can pump the
    /// event loop only when its own promise needs host work.
    pub external_pending: bool,
    /// The fulfilment value or the rejection reason; `undefined` while pending.
    pub value: Value,
    /// Registrations made before the promise settled. Once it settles these
    /// are drained into the microtask queue and the list stays empty:
    /// a later `then` on a settled promise schedules its job immediately.
    pub reactions: Vec<Reaction>,
    /// Set once a rejection has a handler, so an unhandled rejection can be
    /// distinguished from one that was caught.
    pub handled: bool,
}

impl Default for PromiseInner {
    fn default() -> Self {
        Self {
            state: PromiseState::Pending,
            resolution_locked: false,
            external_pending: false,
            value: Value::Undefined,
            reactions: Vec::new(),
            handled: false,
        }
    }
}

/// What a generator body is being resumed *for*.
#[derive(Debug, Clone)]
pub enum GenResume {
    /// A normal `next(v)`: `v` becomes the value of the `yield` expression.
    Next(Option<Value>),
    /// The generator is being abandoned. The `yield` expression returns from
    /// the body instead of producing a value, so the interpreter unwinds it
    /// normally and guest `finally` blocks still run -- which is what
    /// `for...of` + `break` does in JavaScript, via the implicit `return()`.
    Return,
    /// `gen.throw(e)`, or an `await` whose promise rejected: the suspension
    /// point raises `e` instead of producing a value, so guest `try`/`catch`
    /// around it runs.
    Throw(Value),
    /// The body is being abandoned: its last handle was dropped while it was
    /// suspended. The suspension point must return `VmErr::Abandon`
    /// immediately — running no guest `catch`/`finally`/`close` on the way
    /// out — so the coroutine completes with a normal return. This is what
    /// keeps an abandoned body off the platform unwinder entirely (the forced
    /// unwind `Coroutine::drop` would otherwise perform is what faults with
    /// `STATUS_ACCESS_VIOLATION` on Windows).
    Abandon,
}

/// How a generator body finished.
pub enum GenOutcome {
    /// The body ran to completion (or hit `return`), carrying its value.
    Returned(Value),
    /// The body threw. Carries the thrown *value*, not a rendering of it, so
    /// a `catch` on the other side of the coroutine boundary sees the original
    /// error object rather than its `toString`.
    Threw(Value),
    /// The body hit an internal failure — a limit, or a signal that escaped —
    /// which is reported as a message rather than as a guest value.
    Failed(String),
    /// The body was abandoned while suspended (see `GenResume::Abandon`): it
    /// unwound without running guest handlers and returned normally. Only
    /// the initiating `Drop` ever observes this, via [`force_abandon`].
    Abandon,
}

/// Tear down a suspended coroutine without the platform unwinder.
///
/// Resumes it with [`GenResume::Abandon`] until it returns. Every suspend
/// point converts that into `VmErr::Abandon`, which skips all guest-code
/// handlers, so the body unwinds purely by dropping its own frames and the
/// coroutine completes with a normal return — a plain stack switch, which
/// works from any depth on every platform. Letting a suspended coroutine
/// drop instead would run corosensei's forced unwind (a cross-stack panic),
/// which the Windows unwinder cannot walk.
///
/// If the abandon itself panics (e.g. a borrow already held on shared state),
/// the coroutine is leaked rather than dropped: leaking one stack is the safe
/// fallback, because `Drop` implementations must never panic.
#[cfg(stackful_coroutines)]
pub(crate) fn force_abandon(coroutine: GenCoroutine) {
    struct LeakOnPanic<'a>(&'a mut Option<GenCoroutine>);
    impl Drop for LeakOnPanic<'_> {
        fn drop(&mut self) {
            if let Some(coroutine) = self.0.take() {
                std::mem::forget(coroutine);
            }
        }
    }

    let mut slot = Some(coroutine);
    {
        let guard = LeakOnPanic(&mut slot);
        let coroutine = guard.0.as_mut().expect("slot holds the coroutine");
        // Suspend points never yield on `Abandon` — they return the teardown
        // signal instead — so resuming until the first `Return` always
        // terminates.
        while let corosensei::CoroutineResult::Yield(_) = coroutine.resume(GenResume::Abandon) {}

        // Abandon completed without panicking: disarm the leak guard and drop
        // the finished coroutine, which frees its stack with no unwinding.
        let finished = guard.0.take().expect("slot holds the coroutine");
        std::mem::forget(guard);
        drop(finished);
    }
}

/// The coroutine backing one generator.
///
/// Resuming it passes the value given to `next(v)` (as `Option<Value>`) and
/// gets back either a yielded `Value` or the final [`GenOutcome`]. The
/// coroutine runs on its own stack but on the *calling thread*: nothing is
/// sent anywhere, so no `Send` bound and no `unsafe` are involved.
#[cfg(stackful_coroutines)]
pub type GenCoroutine = corosensei::Coroutine<GenResume, Value, GenOutcome>;

/// Handle a generator body uses to suspend itself at a `yield`.
///
/// This exists to keep the one piece of `unsafe` the design needs -- a
/// pointer to a value living on the coroutine's own stack -- in a single
/// audited place, rather than spread across the evaluator.
///
/// # Safety
///
/// `corosensei` hands the body a `&Yielder` borrowed from the coroutine's
/// stack frame, so its lifetime cannot be named by `Interpreter`, which is
/// what needs to reach it at each `yield`. The pointer is sound because:
///
/// 1. the `Yielder` is alive for the whole of the body's execution, and the
///    `Interpreter` holding this handle is *created inside* that body and
///    dropped when it returns or unwinds -- so the handle can never outlive
///    its referent;
/// 2. it is only ever dereferenced on the thread running the coroutine, which
///    is the thread that created it -- `GenYielder` is neither `Send` nor
///    `Sync`, so the compiler enforces that;
/// 3. `suspend` takes `&self`, so no aliasing `&mut` to the `Yielder` exists.
///
/// This is a self-referential borrow expressed as a pointer. It is not the
/// old cross-thread `unsafe impl Send` over `Rc`: nothing is shared between
/// threads here, so there is no refcount to race on.
#[cfg(stackful_coroutines)]
pub struct GenYielder {
    inner: *const corosensei::Yielder<GenResume, Value>,
    /// Pins this handle to one thread: a raw pointer is already `!Send`, and
    /// `PhantomData<*const ()>` makes that explicit and stable.
    _not_send: std::marker::PhantomData<*const ()>,
}

#[cfg(stackful_coroutines)]
impl GenYielder {
    /// Wrap the yielder borrowed from the running coroutine's stack.
    ///
    /// # Safety
    /// The caller must ensure the returned handle is stored only in state
    /// owned by the coroutine body, so it cannot outlive `yielder`.
    pub unsafe fn new(yielder: &corosensei::Yielder<GenResume, Value>) -> Self {
        Self {
            inner: yielder as *const _,
            _not_send: std::marker::PhantomData,
        }
    }

    /// Suspend the generator, handing `value` to the caller of `next()`, and
    /// report why it was resumed.
    pub fn suspend(&self, value: Value) -> GenResume {
        // SAFETY: see the type-level proof. The referent outlives this handle
        // by construction, and this is the thread that created it.
        unsafe { (*self.inner).suspend(value) }
    }
}

/// Mutable state shared across a generator's `next()` calls (behind an `Rc` so
/// clones of the `Value::Generator` observe the same progress).
///
/// Mid-body suspension is implemented with a stackful coroutine: the body runs
/// on its own stack, on the *calling thread*, and `yield` switches back to the
/// caller. That handles infinite generators, `yield` inside loops and
/// conditionals, and `try`/`finally` around a `yield`.
///
/// This replaced an OS-thread implementation. A thread meant moving `Rc`-backed
/// values across a thread boundary under `unsafe impl Send`, and non-atomic
/// refcounts made that a measurable data race no amount of channel discipline
/// closed. A coroutine keeps everything on one thread, so the question does not
/// arise: there is no `Send` bound and no `unsafe` in this path.
pub struct GeneratorInner {
    pub body: Rc<Vec<Statement>>,
    pub closure: Option<Env>,
    pub params: Rc<Vec<Rc<str>>>,
    pub args: Vec<Value>,
    /// The suspended body. `None` before the first `next()`, once the
    /// generator has finished, and -- transiently -- while it is running,
    /// which is how re-entrant `next()` is detected.
    #[cfg(stackful_coroutines)]
    pub coroutine: Option<GenCoroutine>,
    /// Values the body produced, on targets with no stack switching.
    ///
    /// A target without stack switching cannot suspend a running body, so it
    /// runs once to completion and its yields are buffered here for `next()`
    /// to drain. See `call::generator_next` for what that changes, and
    /// `build.rs` for which targets take this path.
    #[cfg(not(stackful_coroutines))]
    pub buffered: std::collections::VecDeque<Value>,
    pub started: bool,
    pub done: bool,
    pub return_value: Option<Value>,
}

#[cfg(stackful_coroutines)]
impl Drop for GeneratorInner {
    /// Tear down a generator abandoned while suspended.
    ///
    /// The suspended coroutine is resumed once with `GenResume::Abandon`
    /// (see [`force_abandon`]), which unwinds the body without running any
    /// guest code — no `finally`, no `catch`, no iterator `close` — and
    /// completes the coroutine with a normal return. That keeps the teardown
    /// off the platform unwinder entirely: dropping a suspended coroutine
    /// would run corosensei's forced unwind, a cross-stack panic the Windows
    /// unwinder cannot walk (`STATUS_ACCESS_VIOLATION`).
    ///
    /// This is deliberately *not* [`GeneratorInner::close`]: closing runs
    /// guest `finally` blocks, and a `Drop` fires at arbitrary points where
    /// running guest code could panic and abort the process. Abandoning runs
    /// none, matching JavaScript, where a generator collected by the GC never
    /// resumes into its handlers.
    fn drop(&mut self) {
        if let Some(coroutine) = self.coroutine.take()
            && !coroutine.done()
        {
            force_abandon(coroutine);
        }
    }
}

#[cfg(stackful_coroutines)]
impl GeneratorInner {
    /// Close a suspended generator the way JavaScript's `return()` does:
    /// resume the body once so its `finally` blocks run, then discard it.
    ///
    /// Closing is an explicit act, performed where the language says an
    /// iterator is closed: leaving a `for...of` early. A generator that is
    /// merely dropped is *not* closed (see the `Drop` impl above): its
    /// `finally` does not run, matching JavaScript, where a generator
    /// collected by the GC never resumes.
    pub fn close(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let Some(mut coroutine) = self.coroutine.take() else {
            return;
        };
        if coroutine.done() {
            return;
        }
        match coroutine.resume(GenResume::Return) {
            corosensei::CoroutineResult::Return(_) => {}
            corosensei::CoroutineResult::Yield(_) => {
                // A `yield` inside the `finally` block: honouring it would
                // let the generator resurrect itself mid-teardown. Abandon
                // the remainder without running further handlers — and
                // without a forced unwind.
                force_abandon(coroutine);
            }
        }
    }
}

impl std::fmt::Debug for GeneratorInner {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "GeneratorInner {{ started: {}, done: {} }}",
            self.started, self.done
        )
    }
}

impl GeneratorInner {
    /// Whether this generator's suspended state holds values the tracer
    /// cannot see. A live coroutine does; buffered yields are traced.
    pub(crate) fn suspends_values(&self) -> bool {
        #[cfg(stackful_coroutines)]
        {
            self.coroutine.is_some()
        }
        #[cfg(not(stackful_coroutines))]
        {
            false
        }
    }
}

impl Value {
    /// Construct a host-backed callable with JavaScript function own
    /// properties. `name` and `length` are non-enumerable, non-writable,
    /// configurable data properties, as for a native JavaScript function.
    pub fn host_function(name: impl Into<Rc<str>>, id: usize) -> Self {
        let name = name.into();
        let properties = crate::heap::tracked(Rc::new(ObjectCell::new_with_default_proto(vec![
            ("length".to_string(), Value::Number(0.0)),
            ("name".to_string(), Value::String(name.to_string())),
        ])));
        let intrinsic_attributes = PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        };
        properties
            .meta
            .borrow_mut()
            .set_attrs("length", intrinsic_attributes);
        properties
            .meta
            .borrow_mut()
            .set_attrs("name", intrinsic_attributes);
        properties.meta.borrow_mut().host_function_id = Some(id);
        Value::HostFunction { name, properties }
    }

    /// Construct a Node-API callback function. `napi_create_function` yields
    /// an ordinary constructable function, including its own `prototype`
    /// property and the prototype object's `constructor` back-reference.
    pub fn napi_callback_function(name: impl Into<Rc<str>>, id: usize) -> Self {
        let function = Self::host_function(name, id);
        let Value::HostFunction { properties, .. } = &function else {
            unreachable!("host_function returns a host-backed function")
        };
        let prototype = Value::object(vec![("constructor".to_string(), function.clone())]);
        if let Value::Object { props } = &prototype {
            props.meta.borrow_mut().set_attrs(
                "constructor",
                PropAttrs {
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        properties
            .borrow_mut()
            .push(("prototype".to_string(), prototype));
        properties.meta.borrow_mut().set_attrs(
            "prototype",
            PropAttrs {
                writable: true,
                enumerable: false,
                configurable: false,
            },
        );
        function
    }

    pub fn host_function_id(&self) -> Option<usize> {
        match self {
            Value::HostFunction { properties, .. } => properties.meta.borrow().host_function_id,
            _ => None,
        }
    }

    /// Apply an inferred property name to a host-backed function while
    /// retaining its bridge identity and shared own-property cell.
    pub fn host_function_named(&self, name: impl Into<Rc<str>>) -> Option<Self> {
        let Value::HostFunction { properties, .. } = self else {
            return None;
        };
        let name = name.into();
        Some(Value::HostFunction {
            name,
            properties: properties.clone(),
        })
    }

    pub fn checked_object(props: Vec<(String, Value)>) -> Result<Self, VmErr> {
        if props.len() > MAX_OBJECT_PROPS {
            return Err(limit_err("Maximum object property count exceeded"));
        }
        Ok(Self::object(props))
    }

    pub fn object(props: Vec<(String, Value)>) -> Self {
        Value::Object {
            props: crate::heap::tracked(Rc::new(ObjectCell::new_with_default_proto(props))),
        }
    }

    /// The detached `arguments` object for a call: indexed properties plus
    /// `length`. Shared by the evaluator's call paths and the VM's frame
    /// seeding, so both tiers bind the identical shape.
    pub fn arguments_object(args: &[Value]) -> Result<Self, crate::error::VmErr> {
        let args_obj = Value::object(
            args.iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect(),
        );
        args_obj.set_prop("length".to_string(), Value::Number(args.len() as f64))?;
        Ok(args_obj)
    }

    pub fn object_with_proto(props: Vec<(String, Value)>, proto: Option<Rc<Value>>) -> Self {
        Value::Object {
            props: crate::heap::tracked(Rc::new(ObjectCell::new(props, proto))),
        }
    }

    /// Create the object wrapper returned by ECMAScript `ToObject` for a
    /// primitive value.
    pub fn boxed_primitive(value: Value) -> Option<Self> {
        let boxed = match &value {
            Value::Bool(value) => BoxedPrimitive::Bool(*value),
            Value::Number(value) => BoxedPrimitive::Number(*value),
            Value::String(value) => BoxedPrimitive::String(value.clone()),
            Value::Symbol(value) => BoxedPrimitive::Symbol(value.clone()),
            Value::BigInt(value) => BoxedPrimitive::BigInt(value.clone()),
            _ => return None,
        };
        let object = Self::object(Vec::new());
        if let Value::Object { props } = &object {
            props.meta.borrow_mut().boxed_primitive = Some(boxed);
        }
        Some(object)
    }

    /// The object's prototype link, or `None` for a null prototype / a
    /// non-object receiver.
    pub fn proto_of(&self) -> Option<Rc<Value>> {
        match self {
            Value::Object { props } => props.proto(),
            Value::Array(array) => array.proto(),
            Value::Class(class) => class.statics.proto(),
            Value::Function(function) => function.properties.proto(),
            Value::HostFunction { properties, .. } => properties.proto(),
            _ => None,
        }
    }

    /// A promise that is already settled — what `Promise.resolve`,
    /// `Promise.reject` and a completed async function hand back.
    pub fn settled_promise(state: PromiseState, value: Value) -> Self {
        Value::Promise(crate::heap::tracked(Rc::new(RefCell::new(PromiseInner {
            state,
            resolution_locked: true,
            external_pending: false,
            value,
            reactions: Vec::new(),
            handled: state != PromiseState::Rejected,
        }))))
    }

    pub fn pending_promise() -> Rc<RefCell<PromiseInner>> {
        crate::heap::tracked(Rc::new(RefCell::new(PromiseInner::default())))
    }

    pub fn array(items: Vec<Value>) -> Self {
        Value::Array(crate::heap::tracked(Rc::new(ArrayCell::new(items))))
    }

    pub fn array_with_presence(items: Vec<Value>, present: Vec<bool>) -> Self {
        Value::Array(crate::heap::tracked(Rc::new(ArrayCell::with_presence(
            items, present,
        ))))
    }

    pub fn checked_array(items: Vec<Value>) -> Result<Self, VmErr> {
        if items.len() > MAX_ARRAY_LEN {
            return Err(limit_err("Maximum array length exceeded"));
        }
        Ok(Self::array(items))
    }

    pub fn checked_array_with_presence(
        items: Vec<Value>,
        present: Vec<bool>,
    ) -> Result<Self, VmErr> {
        if items.len() > MAX_ARRAY_LEN {
            return Err(limit_err("Maximum array length exceeded"));
        }
        Ok(Self::array_with_presence(items, present))
    }

    pub fn checked_string(value: String) -> Result<Self, VmErr> {
        if value.len() > MAX_STRING_LEN {
            return Err(limit_err("Maximum string length exceeded"));
        }
        Ok(Self::String(value))
    }

    /// Read through a live module binding. A value that is not one is
    /// returned unchanged, so this is safe to apply anywhere.
    pub fn deref_binding(&self) -> Value {
        match self {
            Value::Binding(cell) => cell.borrow().clone(),
            other => other.clone(),
        }
    }

    /// The shared promise state, if this is a promise.
    ///
    /// A by-reference accessor: `Value` implements `Drop`, so its payloads
    /// cannot be moved out of a pattern and every caller would otherwise need
    /// a `match … => x.clone()`.
    pub fn as_promise(&self) -> Option<Rc<RefCell<PromiseInner>>> {
        match self {
            Value::Promise(inner) => Some(inner.clone()),
            _ => None,
        }
    }

    /// The shared element storage, if this is an array.
    pub fn as_proxy(&self) -> Option<Rc<ProxyData>> {
        match self {
            Value::Proxy(data) => Some(data.clone()),
            _ => None,
        }
    }

    pub fn as_bigint(&self) -> Option<Rc<crate::bigint::BigInt>> {
        match self {
            Value::BigInt(value) => Some(value.clone()),
            _ => None,
        }
    }

    pub fn as_regexp(&self) -> Option<Rc<RegExpData>> {
        match self {
            Value::RegExp(data) => Some(data.clone()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<Rc<ArrayCell>> {
        match self {
            Value::Array(cell) => Some(cell.clone()),
            _ => None,
        }
    }

    pub fn get_prop(&self, key: &str) -> Option<Value> {
        if let Value::Function(function) = self {
            if key == "prototype" {
                return Some(function.prototype_value(self));
            }
            function.ensure_name_length_properties();
        }
        match self {
            Value::Object { .. }
            | Value::Class(_)
            | Value::Function(_)
            | Value::HostFunction { .. } => {
                // Borrow the receiver until the walk actually descends.
                let mut descended: Option<Value> = None;
                for _ in 0..=MAX_PROTOTYPE_DEPTH {
                    let node = descended.as_ref().unwrap_or(self);
                    let props = match node {
                        Value::Object { props } => props,
                        Value::Class(class) => &class.statics,
                        Value::Function(function) => &function.properties,
                        Value::HostFunction { properties, .. } => properties,
                        _ => return None,
                    };
                    if let Some(value) = props.own_value(key) {
                        return Some(value.deref_binding());
                    }
                    let Some(next) = props.proto() else {
                        break;
                    };
                    descended = Some(next.as_ref().clone());
                }
                match self {
                    Value::Function(_) | Value::HostFunction { .. } => match key {
                        // Function.prototype supplies these after an own
                        // configurable name/length property is deleted.
                        "name" => Some(Value::String(String::new())),
                        "length" => Some(Value::Number(0.0)),
                        _ => crate::builtins::function_method(key),
                    },
                    _ => None,
                }
            }
            Value::Array(cell) => {
                let items = cell.borrow();
                if key == "length" {
                    return Some(Value::Number(items.len() as f64));
                }
                if let Some(idx) = array_index(key)
                    && idx < items.len()
                {
                    return Some(items[idx].clone());
                }
                drop(items);
                cell.named_prop(key)
            }
            Value::String(s) => {
                if key == "length" {
                    return Some(Value::Number(str_char_len(s)));
                }
                if let Ok(idx) = key.parse::<usize>() {
                    return str_char_at(s, idx);
                }
                None
            }
            Value::Error(e) => match key {
                "message" => Some(Value::String(e.message.clone())),
                "name" => Some(Value::String(e.name.clone())),
                "stack" => Some(Value::String(e.stack.clone())),
                "code" => e.code.as_ref().map(|code| Value::String(code.clone())),
                _ => None,
            },
            Value::StringIterator { .. } => None,
            _ => None,
        }
    }

    /// Insert or replace an own property while enforcing the object cap.
    pub fn set_prop(&self, key: String, val: Value) -> Result<(), VmErr> {
        match self {
            Value::Array(cell) => {
                cell.set_named(key, val);
                Ok(())
            }
            Value::Object { props } => set_cell_prop(props, key, val),
            Value::Class(class) => set_cell_prop(&class.statics, key, val),
            Value::Function(function) => {
                function.ensure_name_length_properties();
                set_cell_prop(&function.properties, key, val)
            }
            Value::HostFunction { properties, .. } => set_cell_prop(properties, key, val),
            _ => Ok(()),
        }
    }

    pub fn has_prop(&self, key: &str) -> bool {
        // A proxy without a `has` trap answers for its target. The trap
        // itself is applied by `bin_op`, which can call guest code.
        if let Value::Proxy(proxy) = self {
            return proxy.target.has_prop(key);
        }
        if let Value::Function(function) = self {
            if key == "prototype" {
                function.prototype_value(self);
            } else {
                function.ensure_name_length_properties();
            }
        }
        match self {
            Value::Object { .. }
            | Value::Class(_)
            | Value::Function(_)
            | Value::HostFunction { .. } => {
                // Borrow the receiver until the walk actually descends.
                let mut descended: Option<Value> = None;
                for _ in 0..=MAX_PROTOTYPE_DEPTH {
                    let node = descended.as_ref().unwrap_or(self);
                    let props = match node {
                        Value::Object { props } => props,
                        Value::Class(class) => &class.statics,
                        Value::Function(function) => &function.properties,
                        Value::HostFunction { properties, .. } => properties,
                        _ => return false,
                    };
                    if props.own_index(key).is_some() {
                        return true;
                    }
                    let Some(next) = props.proto() else {
                        break;
                    };
                    descended = Some(next.as_ref().clone());
                }
                match self {
                    Value::Function(_) | Value::HostFunction { .. } => {
                        matches!(key, "name" | "length")
                            || crate::builtins::function_method(key).is_some()
                    }
                    _ => false,
                }
            }
            Value::Array(cell) => {
                key == "length"
                    || array_index(key).is_some_and(|i| cell.has_index(i))
                    || cell.named_prop(key).is_some()
            }
            Value::String(_) => key == "length",
            Value::Error(error) => {
                matches!(key, "message" | "name" | "stack")
                    || (key == "code" && error.code.is_some())
            }
            Value::StringIterator { .. } => false,
            _ => false,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::BigInt(value) => !value.is_zero(),
            Value::String(s) => !s.is_empty(),
            Value::StringIterator { .. } => true,
            Value::Null | Value::Undefined => false,
            _ => true,
        }
    }

    pub fn to_number(&self) -> f64 {
        match self {
            Value::Number(n) => *n,
            Value::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    0.0
                } else {
                    trimmed.parse().unwrap_or(f64::NAN)
                }
            }
            Value::BigInt(value) => value.to_f64(),
            Value::Date(ms) => ms.get(),
            // An array converts through its string form, which is why `[1] * 3`
            // is 3, `[] * 3` is 0 and `[1, 2] * 3` is NaN.
            Value::Array(cell) => {
                let items = cell.borrow();
                match items.len() {
                    0 => 0.0,
                    1 => items[0].to_number(),
                    _ => f64::NAN,
                }
            }
            Value::StringIterator { .. } => 0.0,
            Value::Null => 0.0,
            // An object has no numeric value: `{} * 3` is NaN, not 0.
            Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
                Some(BoxedPrimitive::Number(value)) => *value,
                Some(BoxedPrimitive::Bool(value)) => {
                    if *value {
                        1.0
                    } else {
                        0.0
                    }
                }
                Some(BoxedPrimitive::String(value)) => {
                    let trimmed = value.trim();
                    if trimmed.is_empty() {
                        0.0
                    } else {
                        trimmed.parse().unwrap_or(f64::NAN)
                    }
                }
                Some(BoxedPrimitive::BigInt(value)) => value.to_f64(),
                Some(BoxedPrimitive::Symbol(_)) | None => f64::NAN,
            },
            Value::Undefined | Value::Error(_) => f64::NAN,
            _ => 0.0,
        }
    }

    /// Move the direct child `Value`s out of `self` into `work`, leaving
    /// shallow placeholders behind. See the `Drop` impl for why.
    fn take_children(&mut self, work: &mut Vec<Value>) {
        match self {
            Value::Array(items) => {
                // Only drain when we own the buffer outright; a shared Rc
                // keeps its contents until the last reference drops.
                if Rc::strong_count(items) == 1
                    && let Some(cell) = Rc::get_mut(items)
                {
                    work.append(cell.elements_mut());
                    work.extend(cell.named.get_mut().drain(..).map(|(_, v)| v));
                }
            }
            Value::Object { props } => {
                if Rc::strong_count(props) == 1
                    && let Some(cell) = Rc::get_mut(props)
                {
                    work.extend(cell.slots_mut().drain(..).map(|(_, v)| v));
                    if let Some(p) = cell.meta.get_mut().proto.take()
                        && let Ok(inner) = Rc::try_unwrap(p)
                    {
                        work.push(inner);
                    }
                }
            }
            Value::Function(fd) => {
                // A clone shares the function payload. Drain its nested
                // values only when this is the final owner.
                if let Some(fd) = Rc::get_mut(fd) {
                    if let Some(env) = fd.closure.take() {
                        crate::interpreter::Environment::drain_chain(env, work);
                    }
                    if let Some(bound) = fd.bound.take()
                        && let Ok(mut bound) = Rc::try_unwrap(bound)
                    {
                        work.push(std::mem::replace(&mut bound.target, Value::Undefined));
                        work.push(std::mem::replace(&mut bound.this_value, Value::Undefined));
                        if let Ok(mut arguments) = Rc::try_unwrap(bound.arguments) {
                            work.append(&mut arguments);
                        }
                    }
                }
            }
            Value::Class(cd) => {
                work.push(std::mem::replace(cd.constructor.as_mut(), Value::Undefined));
                if Rc::strong_count(&cd.prototype) == 1
                    && let Some(p) = Rc::get_mut(&mut cd.prototype)
                {
                    p.take_children(work);
                }
                if Rc::strong_count(&cd.statics) == 1
                    && let Some(cell) = Rc::get_mut(&mut cd.statics)
                {
                    work.extend(cell.slots_mut().drain(..).map(|(_, v)| v));
                    if let Some(prototype) = cell.meta.get_mut().proto.take()
                        && let Ok(inner) = Rc::try_unwrap(prototype)
                    {
                        work.push(inner);
                    }
                }
            }
            Value::Binding(cell) => {
                if Rc::strong_count(cell) == 1
                    && let Some(inner) = Rc::get_mut(cell)
                {
                    work.push(std::mem::replace(inner.get_mut(), Value::Undefined));
                }
            }
            Value::Promise(inner) => {
                if Rc::strong_count(inner) == 1
                    && let Some(cell) = Rc::get_mut(inner)
                {
                    let inner = cell.get_mut();
                    work.push(std::mem::replace(&mut inner.value, Value::Undefined));
                    for reaction in inner.reactions.drain(..) {
                        work.push(reaction.on_fulfilled);
                        work.push(reaction.on_rejected);
                    }
                }
            }
            Value::Generator { inner } => {
                if Rc::strong_count(inner) == 1
                    && let Some(cell) = Rc::get_mut(inner)
                {
                    let g = cell.get_mut();
                    work.append(&mut g.args);
                    if let Some(v) = g.return_value.take() {
                        work.push(v);
                    }
                    if let Some(env) = g.closure.take() {
                        crate::interpreter::Environment::drain_chain(env, work);
                    }
                }
            }
            // All remaining variants hold no heap-nested `Value`s.
            _ => {}
        }
    }
}

/// Dropping a deeply nested value (guest code can build `a = [a]` a million
/// times in a loop) would recurse once per nesting level in the derived
/// `Drop` glue and overflow the native stack *during teardown* — after the
/// guest code already finished successfully. This iterative implementation
/// drains children onto an explicit work stack instead, so teardown depth is
/// always O(1) regardless of structure depth. Cyclic `Rc` graphs (which Rust
/// would simply leak) are skipped via the strong-count checks, so they can
/// neither recurse infinitely nor crash.
impl Drop for Value {
    fn drop(&mut self) {
        let mut work: Vec<Value> = Vec::new();
        self.take_children(&mut work);
        while let Some(mut v) = work.pop() {
            v.take_children(&mut work);
            // `v` drops here with its children already removed: a shallow,
            // non-recursive drop.
        }
    }
}

#[cfg(all(test, target_has_atomic = "32"))]
mod shared_array_buffer_tests {
    use super::SharedBuffer;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn shared_backing_is_aligned_stable_and_clones_share_bytes() {
        let buffer = SharedBuffer::zeroed(12).unwrap();
        let data = buffer.data_ptr();
        assert_eq!(data as usize % std::mem::align_of::<AtomicU32>(), 0);

        // Node-API returns this pointer to addons which use native atomics.
        // Check both aliasing and pointer stability through a cloned SAB.
        let word = unsafe { &*data.cast::<AtomicU32>().add(1) };
        word.store(0x1234_5678, Ordering::SeqCst);
        let clone = buffer.shared_clone();
        assert_ne!(buffer.identity(), clone.identity());
        assert_eq!(buffer.data_ptr(), clone.data_ptr());
        assert_eq!(
            clone.read(4, 4).unwrap(),
            0x1234_5678_u32.to_ne_bytes().to_vec()
        );
        assert!(clone.write(4, &[1, 2, 3, 4]));
        assert_eq!(word.load(Ordering::SeqCst).to_ne_bytes(), [1, 2, 3, 4]);
    }
}

#[cfg(test)]
mod array_index_tests {
    use super::{Value, array_index, str_char_at, str_char_len};

    #[test]
    fn canonical_indices_parse() {
        assert_eq!(array_index("0"), Some(0));
        assert_eq!(array_index("1"), Some(1));
        assert_eq!(array_index("42"), Some(42));
        assert_eq!(array_index("262143"), Some(262143));
        assert_eq!(array_index("4294967294"), Some(4294967294));
    }

    #[test]
    fn non_canonical_names_are_ordinary_properties() {
        for key in [
            "",
            "01",
            "00",
            "4294967295",
            "4294967296",
            "99999999999999999999",
            "1e3",
            "+1",
            "-1",
            " 1",
            "1 ",
            "length",
            "push",
            "１２", // full-width digits are not ASCII digits
        ] {
            assert_eq!(array_index(key), None, "key {key:?}");
        }
    }

    #[test]
    fn string_helpers_match_char_semantics() {
        assert_eq!(str_char_len("hello"), 5.0);
        assert_eq!(str_char_len("héllo"), 5.0);
        assert_eq!(str_char_len("😀😀"), 2.0);
        assert_eq!(str_char_len(""), 0.0);
        assert!(matches!(str_char_at("hello", 1), Some(Value::String(ref s)) if s == "e"));
        assert!(matches!(str_char_at("hello", 4), Some(Value::String(ref s)) if s == "o"));
        assert!(matches!(str_char_at("héllo", 1), Some(Value::String(ref s)) if s == "é"));
        assert!(matches!(str_char_at("😀x", 1), Some(Value::String(ref s)) if s == "x"));
        assert!(str_char_at("hi", 2).is_none());
        assert!(str_char_at("", 0).is_none());
    }
}
