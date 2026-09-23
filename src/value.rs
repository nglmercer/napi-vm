use std::cell::{Ref, RefCell, RefMut};
use std::ptr::NonNull;
use std::rc::Rc;

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
pub fn array_index(key: &str) -> Option<usize> {
    let index = key.parse::<usize>().ok()?;
    (index.to_string() == key && index < u32::MAX as usize).then_some(index)
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
    /// Whether `proto == None` represents the default JavaScript object
    /// prototype. The interpreter does not currently materialize that built-in
    /// prototype, but bridges need to distinguish it from an explicit null
    /// prototype.
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
}

impl ArrayCell {
    pub fn new(elements: Vec<Value>) -> Self {
        Self {
            elements: RefCell::new(elements),
            present: RefCell::new(None),
            named: RefCell::new(Vec::new()),
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
        }
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

    pub fn set_named(&self, key: String, value: Value) {
        let mut named = self.named.borrow_mut();
        match named.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = value,
            None => named.push((key, value)),
        }
    }

    /// Uncontended access to the elements, for the iterative `Drop`.
    pub fn elements_mut(&mut self) -> &mut Vec<Value> {
        self.elements.get_mut()
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
}

impl ObjectCell {
    pub fn new(props: Vec<(String, Value)>, proto: Option<Rc<Value>>) -> Self {
        Self {
            slots: RefCell::new(props),
            meta: RefCell::new(ObjectMeta {
                proto,
                ..ObjectMeta::default()
            }),
        }
    }

    pub fn new_with_default_proto(props: Vec<(String, Value)>) -> Self {
        Self {
            slots: RefCell::new(props),
            meta: RefCell::new(ObjectMeta {
                uses_default_prototype: true,
                ..ObjectMeta::default()
            }),
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
}

impl std::ops::Deref for ObjectCell {
    type Target = RefCell<Vec<(String, Value)>>;
    fn deref(&self) -> &Self::Target {
        &self.slots
    }
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

/// Payload of `Value::Function`, boxed so the enum itself stays small.
#[derive(Debug, Clone)]
pub struct FunctionData {
    /// Shared identity for this function object. Cloning a VM `Value` keeps the
    /// same identity, while evaluating the same function expression again
    /// creates a distinct one.
    pub identity: Rc<u8>,
    pub name: Option<Rc<str>>,
    // Shared (`Rc`) so closures created in hot loops reference the same AST
    // instead of deep-cloning the parameter list and body on every creation.
    // Param names are `Rc<str>` so binding them in a call frame is a refcount
    // bump, not a heap allocation.
    pub params: Rc<Vec<Rc<str>>>,
    pub body: Rc<Vec<Statement>>,
    pub closure: Option<Env>,
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
    /// Whether the body references `arguments`. Frames for functions that
    /// never read it skip building the (detached) arguments object.
    pub uses_arguments: bool,
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
}

impl BufferStorage {
    unsafe fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::External { data, length } => unsafe {
                std::slice::from_raw_parts(data.as_ptr(), *length)
            },
        }
    }

    unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::External { data, length } => unsafe {
                std::slice::from_raw_parts_mut(data.as_ptr(), *length)
            },
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
    pub buffer: Buffer,
    pub byte_offset: usize,
    /// Element count for a typed array; *byte* count for a `DataView`.
    pub length: usize,
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
/// read. Function/class/error payloads live behind a `Box` — constructing one
/// allocates, but those are rare next to number/string/identifier traffic.
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
    Function(Box<FunctionData>),
    NativeFunction {
        name: Rc<str>,
        callable: fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>,
    },
    /// A function implemented on the host (Node.js) side, reachable from the VM.
    /// Calling it dispatches through the interpreter's `HostBridge` using `id`,
    /// which the bridge maps to a persisted JavaScript function reference.
    HostFunction {
        name: Rc<str>,
        id: usize,
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

impl Value {
    pub fn checked_object(props: Vec<(String, Value)>) -> Result<Self, VmErr> {
        if props.len() > MAX_OBJECT_PROPS {
            return Err(limit_err("Maximum object property count exceeded"));
        }
        Ok(Self::object(props))
    }

    pub fn object(props: Vec<(String, Value)>) -> Self {
        Value::Object {
            props: Rc::new(ObjectCell::new_with_default_proto(props)),
        }
    }

    pub fn object_with_proto(props: Vec<(String, Value)>, proto: Option<Rc<Value>>) -> Self {
        Value::Object {
            props: Rc::new(ObjectCell::new(props, proto)),
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
            Value::Class(class) => class.statics.proto(),
            _ => None,
        }
    }

    /// A promise that is already settled — what `Promise.resolve`,
    /// `Promise.reject` and a completed async function hand back.
    pub fn settled_promise(state: PromiseState, value: Value) -> Self {
        Value::Promise(Rc::new(RefCell::new(PromiseInner {
            state,
            resolution_locked: true,
            external_pending: false,
            value,
            reactions: Vec::new(),
            handled: state != PromiseState::Rejected,
        })))
    }

    pub fn pending_promise() -> Rc<RefCell<PromiseInner>> {
        Rc::new(RefCell::new(PromiseInner::default()))
    }

    pub fn array(items: Vec<Value>) -> Self {
        Value::Array(Rc::new(ArrayCell::new(items)))
    }

    pub fn array_with_presence(items: Vec<Value>, present: Vec<bool>) -> Self {
        Value::Array(Rc::new(ArrayCell::with_presence(items, present)))
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
        match self {
            Value::Object { .. } | Value::Class(_) => {
                let mut current = self.clone();
                for _ in 0..=MAX_PROTOTYPE_DEPTH {
                    let props = match &current {
                        Value::Object { props } => props,
                        Value::Class(class) => &class.statics,
                        _ => return None,
                    };
                    if let Some((_, value)) = props.borrow().iter().find(|(name, _)| name == key) {
                        return Some(value.deref_binding());
                    }
                    let next = props.proto()?;
                    current = next.as_ref().clone();
                }
                None
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
                    return Some(Value::Number(s.chars().count() as f64));
                }
                if let Ok(idx) = key.parse::<usize>() {
                    return s.chars().nth(idx).map(|c| Value::String(c.to_string()));
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
        if let Value::Array(cell) = self {
            cell.set_named(key, val);
            return Ok(());
        }
        if let Value::Object { props } = self {
            let writable = props.meta.borrow().attrs_of(&key).writable;
            let mut slots = props.borrow_mut();
            for (k, v) in slots.iter_mut() {
                if k == &key {
                    // A non-writable property silently ignores the write, the
                    // way a sloppy-mode assignment does.
                    if writable {
                        *v = val;
                    }
                    return Ok(());
                }
            }
            if props.meta.borrow().non_extensible {
                return Ok(());
            }
            if slots.len() >= MAX_OBJECT_PROPS {
                return Err(limit_err("Maximum object property count exceeded"));
            }
            slots.push((key, val));
            return Ok(());
        }
        if let Value::Class(class) = self {
            let writable = class.statics.meta.borrow().attrs_of(&key).writable;
            let mut slots = class.statics.borrow_mut();
            for (name, value) in slots.iter_mut() {
                if name == &key {
                    if writable {
                        *value = val;
                    }
                    return Ok(());
                }
            }
            if class.statics.meta.borrow().non_extensible {
                return Ok(());
            }
            if slots.len() >= MAX_OBJECT_PROPS {
                return Err(limit_err("Maximum object property count exceeded"));
            }
            slots.push((key, val));
        }
        Ok(())
    }

    pub fn has_prop(&self, key: &str) -> bool {
        match self {
            // A proxy without a `has` trap answers for its target. The trap
            // itself is applied by `bin_op`, which can call guest code.
            Value::Proxy(proxy) => proxy.target.has_prop(key),
            Value::Object { .. } | Value::Class(_) => {
                let mut current = self.clone();
                for _ in 0..=MAX_PROTOTYPE_DEPTH {
                    let props = match &current {
                        Value::Object { props } => props,
                        Value::Class(class) => &class.statics,
                        _ => return false,
                    };
                    if props.borrow().iter().any(|(name, _)| name == key) {
                        return true;
                    }
                    let Some(next) = props.proto() else {
                        return false;
                    };
                    current = next.as_ref().clone();
                }
                false
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
                if let Some(env) = fd.closure.take() {
                    crate::interpreter::Environment::drain_chain(env, work);
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
