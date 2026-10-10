//! `Map`, `Set`, `WeakMap` and `WeakSet`.
//!
//! Entries live in an insertion-ordered `Vec` on the collection object, under
//! an internal slot. Keys compare by `SameValueZero` — reference identity for
//! objects, value equality for primitives, with `NaN` equal to itself — which
//! is what `===` cannot express and what distinguishes a `Map` key from a
//! property name.
//!
//! Lookup is a linear scan. That is honest for the sizes a sandboxed script
//! works with and avoids hashing values whose identity is an `Rc` address;
//! the array and object caps bound the worst case.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter, strict_equals};
use crate::value::weak::{WeakStorage, WeakTarget};
use crate::value::{ArrayCell, CollectionKind as Kind, ObjectCell, Value};

/// Slot holding a collection's entries: `[key, value]` pairs for a map,
/// `[value, value]` for a set, so one storage shape serves both.
const ENTRIES_SLOT: &str = "__symbol_entries__";
/// Slot marking which kind of collection an object is, so the methods can
/// report the right `TypeError` and render the right string.
const KIND_SLOT: &str = "__symbol_collection__";

pub(super) fn install(e: &mut Environment) {
    for (name, kind) in [
        ("Map", Kind::Map),
        ("Set", Kind::Set),
        ("WeakMap", Kind::WeakMap),
        ("WeakSet", Kind::WeakSet),
    ] {
        let Some(namespace) = e.get(name) else {
            continue;
        };
        super::make_callable(&namespace, require_new, Some(kind.constructor()));
        // Each VM gets its own prototype object: instances inherit from the
        // namespace's live `.prototype` (identity matters for `instanceof`),
        // and the `constructor` back-link must point at this VM's namespace,
        // not a cross-VM shared object.
        let function_prototype = e.get("Function").and_then(|f| f.get_prop("prototype"));
        let proto =
            build_prototype(kind, function_prototype.clone()).expect("collection prototype");
        proto
            .set_prop("constructor".to_string(), namespace.clone())
            .expect("collection prototype constructor");
        if let Value::Object { props } = &proto {
            props.meta.borrow_mut().set_attrs(
                "constructor",
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
        }
        super::set_builtin_constructor_prototype(e, &namespace, proto);
        namespace
            .set_prop("name".into(), Value::String(name.into()))
            .expect("constructor name");
        namespace
            .set_prop("length".into(), Value::Number(0.))
            .expect("constructor length");
        if let Value::Object { props } = &namespace {
            for key in ["name", "length"] {
                props.meta.borrow_mut().set_attrs(
                    key,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
            props.meta.borrow_mut().set_attrs(
                "prototype",
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
        }
    }
}

impl Kind {
    fn constructor(self) -> super::NativeFn {
        match self {
            Kind::Map => new_map,
            Kind::Set => new_set,
            Kind::WeakMap => new_weak_map,
            Kind::WeakSet => new_weak_set,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Kind::Map => "Map",
            Kind::Set => "Set",
            Kind::WeakMap => "WeakMap",
            Kind::WeakSet => "WeakSet",
        }
    }

    fn keyed(self) -> bool {
        matches!(self, Kind::Map | Kind::WeakMap)
    }
}

/// `SameValueZero`: `===` except that `NaN` matches itself.
///
/// This is the comparison `Map`, `Set` and `Array.prototype.includes` use, and
/// the reason `new Set([NaN, NaN]).size` is 1.
fn same_value_zero(a: &Value, b: &Value) -> bool {
    if let (Value::Number(x), Value::Number(y)) = (a, b)
        && x.is_nan()
        && y.is_nan()
    {
        return true;
    }
    strict_equals(a, b)
}

fn entries_of(this: &Value) -> Option<Rc<ArrayCell>> {
    this.get_prop(ENTRIES_SLOT)?.as_array()
}

fn kind_of(this: &Value) -> Option<Kind> {
    let Value::Object { props } = this else {
        return None;
    };
    props.meta.borrow().collection_kind
}

/// The ECMAScript `Object.prototype.toString` brand for guest collections.
pub fn collection_tag(value: &Value) -> Option<&'static str> {
    kind_of(value).map(Kind::tag)
}

fn require(this: &Value, method: &str) -> Result<Rc<ArrayCell>, VmErr> {
    entries_of(this).ok_or_else(|| {
        VmErr::Msg(format!(
            "TypeError: {} called on an incompatible receiver",
            method
        ))
    })
}

fn weak_entries(this: &Value) -> Option<Rc<ObjectCell>> {
    let Value::Object { props } = this else {
        return None;
    };
    if matches!(*props.weak.borrow(), WeakStorage::Map(_)) {
        Some(props.clone())
    } else {
        None
    }
}
fn weak_insert(
    interp: &Interpreter,
    props: &ObjectCell,
    key: &Value,
    value: Value,
) -> Result<(), VmErr> {
    let target = WeakTarget::new(key, &interp.persistent_global).ok_or_else(|| {
        VmErr::Msg(
            "TypeError: Weak collection key must be an object or a non-registered symbol".into(),
        )
    })?;
    let mut storage = props.weak.borrow_mut();
    let WeakStorage::Map(entries) = &mut *storage else {
        unreachable!("weak map receiver")
    };
    if let Some((_, slot)) = entries.iter_mut().find(|(target, _)| target.matches(key)) {
        *slot = value;
    } else {
        if entries.len() >= crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum collection size exceeded"));
        }
        entries.push((target, value));
    }
    Ok(())
}
/// Index of the entry whose key matches, if any.
fn position(entries: &Rc<ArrayCell>, key: &Value) -> Option<usize> {
    entries.borrow().iter().position(|entry| {
        entry
            .get_prop("0")
            .is_some_and(|candidate| same_value_zero(&candidate, key))
    })
}

fn entry(key: Value, value: Value) -> Value {
    Value::array(vec![key, value])
}

thread_local! {
    /// One prototype per kind, built on first use and shared by every
    /// instance. Methods live there rather than on the instance, so
    /// `Object.keys(map)` is empty — as it is in a real engine.
    static PROTOTYPES: RefCell<Vec<(&'static str, Rc<Value>, crate::heap::RootId)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
#[cfg(feature = "napi")]
pub(crate) struct CollectionContext(Vec<(&'static str, Rc<Value>, crate::heap::RootId)>);
#[cfg(feature = "napi")]
impl CollectionContext {
    pub(crate) fn swap_active(&mut self) {
        PROTOTYPES.with(|p| std::mem::swap(&mut *p.borrow_mut(), &mut self.0));
    }
}

/// Called only after this arena's final interpreter has been dropped.
pub(crate) fn clear_collection_cache() {
    PROTOTYPES.with(|protos| {
        for (_, _, pin) in protos.borrow_mut().drain(..) {
            crate::heap::remove_root(pin);
        }
    });
}

/// A fresh prototype object: methods, the `size` getter and the iterator.
/// The caller decides whether it is cached or installed per-VM.
fn build_prototype(kind: Kind, function_prototype: Option<Value>) -> Result<Value, VmErr> {
    let proto = Value::object(vec![]);
    for (name, callable) in methods(kind) {
        let length = match name {
            "set" | "getOrInsert" | "getOrInsertComputed" => 2,
            "clear" | "keys" | "values" | "entries" => 0,
            _ => 1,
        };
        proto.set_prop(
            name.to_string(),
            super::native_method(name, length, callable, function_prototype.clone()),
        )?;
        if let Value::Object { props } = &proto {
            props.meta.borrow_mut().set_attrs(
                name,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
        }
    }
    // `size` is a getter, so it tracks mutation instead of freezing at
    // construction time. The `get ` name prefix is what the property resolver
    // recognizes as an accessor.
    if !matches!(kind, Kind::WeakMap | Kind::WeakSet) {
        super::object::define_property(
            &proto,
            "size",
            &Value::descriptor_record(vec![
                ("get".into(), super::nf("get size", size_getter)),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )?;
        let iterator = proto
            .get_prop(if kind == Kind::Map {
                "entries"
            } else {
                "values"
            })
            .expect("iterator method");
        if kind == Kind::Set {
            proto.set_prop("keys".into(), iterator.clone())?;
        }
        proto.set_prop(
            crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string(),
            iterator,
        )?;
        if let Value::Object { props } = &proto {
            props.meta.borrow_mut().set_attrs(
                "size",
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
            props.meta.borrow_mut().set_attrs(
                crate::interpreter::SYMBOL_ITERATOR_SLOT,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
            if let Some(Value::Symbol(ref symbol)) = super::well_known("iterator") {
                props
                    .meta
                    .borrow_mut()
                    .set_symbol_key(crate::interpreter::SYMBOL_ITERATOR_SLOT, symbol.clone());
            }
        }
    }
    if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        proto.set_prop(key.clone(), Value::String(kind.tag().into()))?;
        if let Value::Object { props } = &proto {
            let mut meta = props.meta.borrow_mut();
            meta.set_symbol_key(&key, symbol.clone());
            meta.set_attrs(
                &key,
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }
    Ok(proto)
}

fn prototype_for(kind: Kind) -> Result<Rc<Value>, VmErr> {
    if let Some(existing) = PROTOTYPES.with(|protos| {
        protos
            .borrow()
            .iter()
            .find(|(tag, _, _)| *tag == kind.tag())
            .map(|(_, proto, _)| proto.clone())
    }) {
        return Ok(existing);
    }
    let proto = Rc::new(build_prototype(kind, None)?);
    let pin = crate::heap::add_root((*proto).clone());
    PROTOTYPES.with(|protos| protos.borrow_mut().push((kind.tag(), proto.clone(), pin)));
    Ok(proto)
}

/// The `[[Prototype]]` for a new instance: the namespace's live `.prototype`
/// (OrdinaryCreateFromConstructor), falling back to the intrinsic default
/// when a guest replaced it with a non-object.
fn instance_proto(interp: &mut Interpreter, kind: Kind) -> Result<Rc<Value>, VmErr> {
    // The global lookup ends before `member` runs: the member read can
    // execute guest getters, which need the interpreter mutably.
    let namespace = interp
        .new_target_stack
        .last()
        .cloned()
        .or_else(|| interp.persistent_global.borrow().intrinsic(kind.tag()));
    if let Some(namespace) = namespace
        && let Some(prototype) = interp.constructor_prototype(&namespace, kind.tag())?
    {
        return Ok(prototype);
    }
    prototype_for(kind)
}

/// A `super()` receiver that is already a subclass instance — an ordinary
/// object, not yet a collection, whose chain reaches the namespace's live
/// `.prototype` — is initialized in place, so `class S extends Set` keeps its
/// more-derived prototype. Anything else (the namespace itself on a direct
/// call, the global object, an unrelated receiver) keeps the historical
/// fresh object.
fn in_place_target(
    interp: &mut Interpreter,
    kind: Kind,
    this: &Value,
) -> Result<Option<Value>, VmErr> {
    if !matches!(this, Value::Object { .. }) || entries_of(this).is_some() {
        return Ok(None);
    }
    let namespace = interp.persistent_global.borrow().intrinsic(kind.tag());
    let Some(namespace) = namespace else {
        return Ok(None);
    };
    let Ok(proto) = interp.member(&namespace, "prototype") else {
        return Ok(None);
    };
    if !matches!(proto, Value::Object { .. }) {
        return Ok(None);
    }
    let mut current = interp.get_prototype_of(this)?;
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        if matches!(current, Value::Null) {
            return Ok(None);
        }
        if strict_equals(&current, &proto) {
            return Ok(Some(this.clone()));
        }
        current = interp.get_prototype_of(&current)?;
    }
    Ok(None)
}

/// Build one collection, seeded from an optional iterable argument.
fn construct(
    interp: &mut Interpreter,
    kind: Kind,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let collection = match in_place_target(interp, kind, &this)? {
        Some(target) => {
            target.set_prop(ENTRIES_SLOT.to_string(), Value::array(vec![]))?;
            target.set_prop(
                KIND_SLOT.to_string(),
                Value::String((kind.tag().to_string()).into()),
            )?;
            target
        }
        None => Value::object_with_proto(
            vec![
                (ENTRIES_SLOT.to_string(), Value::array(vec![])),
                (
                    KIND_SLOT.to_string(),
                    Value::String((kind.tag().to_string()).into()),
                ),
            ],
            Some(instance_proto(interp, kind)?),
        ),
    };

    if let Value::Object { props } = &collection {
        props.meta.borrow_mut().collection_kind = Some(kind);
        if matches!(kind, Kind::WeakMap | Kind::WeakSet) {
            *props.weak.borrow_mut() = WeakStorage::Map(Vec::new());
        }
    }
    if let Some(source) = args.first()
        && !matches!(source, Value::Undefined | Value::Null)
    {
        // The adder is captured once before obtaining the iterator, including
        // for empty iterables. Overrides and accessor failures are observable.
        let adder = interp.member(&collection, if kind.keyed() { "set" } else { "add" })?;
        if !crate::interpreter::is_callable_value(&adder) {
            return Err(VmErr::Msg(
                "TypeError: Collection adder is not callable".into(),
            ));
        }
        let symbol = super::well_known("iterator").expect("Symbol.iterator");
        let method = interp.get_prop_value(source, &symbol)?;
        if !crate::interpreter::is_callable_value(&method) {
            return Err(VmErr::Msg("TypeError: Value is not iterable".into()));
        }
        let iterator = interp.call_this(&method, source.clone(), vec![])?;
        if !crate::interpreter::call::is_js_object(&iterator) {
            return Err(VmErr::Msg("TypeError: Iterator must be an object".into()));
        }
        let next = interp.member(&iterator, "next")?;
        loop {
            interp.consume_loop()?;
            let step = interp.call_this(&next, iterator.clone(), vec![])?;
            if !crate::interpreter::call::is_js_object(&step) {
                return Err(VmErr::Msg(
                    "TypeError: Iterator result must be an object".into(),
                ));
            }
            if interp.member(&step, "done")?.is_truthy() {
                break;
            }
            let item = interp.member(&step, "value")?;
            let inserted = (|| {
                let arguments = if kind.keyed() {
                    if !crate::interpreter::call::is_js_object(&item) {
                        return Err(VmErr::Msg(
                            "TypeError: Iterator entry must be an object".into(),
                        ));
                    }
                    vec![interp.member(&item, "0")?, interp.member(&item, "1")?]
                } else {
                    vec![item]
                };
                interp.call_this(&adder, collection.clone(), arguments)
            })();
            if let Err(error) = inserted {
                // IteratorClose with a throw completion preserves the original
                // exception even when getting/calling return also throws.
                let _ = interp.close_guest_iterator(&iterator, false);
                return Err(error);
            }
        }
    }
    Ok(collection)
}

macro_rules! branded {
    ($name:ident, $kind:ident, $implementation:ident) => {
        fn $name(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
            if kind_of(&this) != Some(Kind::$kind) {
                return Err(VmErr::Msg(
                    "TypeError: Collection method called on incompatible receiver".into(),
                ));
            }
            $implementation(interp, this, args)
        }
    };
}
branded!(map_has, Map, collection_has);
branded!(map_delete, Map, collection_delete);
branded!(map_get_branded, Map, map_get);
branded!(map_set_branded, Map, map_set);
branded!(map_clear, Map, collection_clear);
branded!(map_for_each, Map, collection_for_each);
branded!(map_keys, Map, collection_keys);
branded!(map_values, Map, collection_values);
branded!(map_entries, Map, collection_entries);
branded!(weak_map_has, WeakMap, collection_has);
branded!(weak_map_delete, WeakMap, collection_delete);
branded!(weak_map_get, WeakMap, map_get);
branded!(weak_map_set, WeakMap, map_set);
branded!(set_has, Set, collection_has);
branded!(set_delete, Set, collection_delete);
branded!(set_add_branded, Set, set_add);
branded!(set_clear, Set, collection_clear);
branded!(set_for_each, Set, collection_for_each);
branded!(set_keys, Set, collection_keys);
branded!(set_values, Set, collection_values);
branded!(set_entries, Set, collection_entries);
branded!(weak_set_has, WeakSet, collection_has);
branded!(weak_set_delete, WeakSet, collection_delete);
branded!(weak_set_add, WeakSet, set_add);
branded!(map_get_or_insert, Map, get_or_insert);
branded!(weak_map_get_or_insert, WeakMap, get_or_insert);
branded!(map_get_or_insert_computed, Map, get_or_insert_computed);
branded!(
    weak_map_get_or_insert_computed,
    WeakMap,
    get_or_insert_computed
);

fn methods(kind: Kind) -> Vec<(&'static str, super::NativeFn)> {
    use Kind::*;
    match kind {
        Map => vec![
            ("has", map_has),
            ("delete", map_delete),
            ("get", map_get_branded),
            ("set", map_set_branded),
            ("getOrInsert", map_get_or_insert),
            ("getOrInsertComputed", map_get_or_insert_computed),
            ("clear", map_clear),
            ("forEach", map_for_each),
            ("keys", map_keys),
            ("values", map_values),
            ("entries", map_entries),
        ],
        WeakMap => vec![
            ("has", weak_map_has),
            ("delete", weak_map_delete),
            ("get", weak_map_get),
            ("set", weak_map_set),
            ("getOrInsert", weak_map_get_or_insert),
            ("getOrInsertComputed", weak_map_get_or_insert_computed),
        ],
        Set => vec![
            ("has", set_has),
            ("delete", set_delete),
            ("add", set_add_branded),
            ("clear", set_clear),
            ("forEach", set_for_each),
            ("keys", set_keys),
            ("values", set_values),
            ("entries", set_entries),
        ],
        WeakSet => vec![
            ("has", weak_set_has),
            ("delete", weak_set_delete),
            ("add", weak_set_add),
        ],
    }
}

fn existing_value(this: &Value, key: &Value) -> Option<Value> {
    if let Some(props) = weak_entries(this) {
        let storage = props.weak.borrow();
        let WeakStorage::Map(entries) = &*storage else {
            unreachable!()
        };
        return entries
            .iter()
            .find(|(target, _)| target.matches(key))
            .map(|(_, value)| value.clone());
    }
    let entries = entries_of(this)?;
    let index = position(&entries, key)?;
    entries.borrow()[index].get_prop("1")
}
fn upsert_key(interp: &Interpreter, this: &Value, args: &[Value]) -> Result<Value, VmErr> {
    let mut key = args.first().cloned().unwrap_or(Value::Undefined);
    if kind_of(this) == Some(Kind::WeakMap) {
        if WeakTarget::new(&key, &interp.persistent_global).is_none() {
            return Err(VmErr::Msg(
                "TypeError: WeakMap key must be weakly holdable".into(),
            ));
        }
    } else if matches!(key,Value::Number(n) if n==0.) {
        key = Value::Number(0.);
    }
    Ok(key)
}
fn get_or_insert(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let key = upsert_key(interp, &this, &args)?;
    if let Some(value) = existing_value(&this, &key) {
        return Ok(value);
    }
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    map_set(interp, this, vec![key, value.clone()])?;
    Ok(value)
}
fn get_or_insert_computed(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = upsert_key(interp, &this, &args)?;
    let callback = args.get(1).cloned().unwrap_or(Value::Undefined);
    if !crate::interpreter::is_callable_value(&callback) {
        return Err(VmErr::Msg("TypeError: Callback must be callable".into()));
    }
    if let Some(value) = existing_value(&this, &key) {
        return Ok(value);
    }
    let value = interp.call_this(&callback, Value::Undefined, vec![key.clone()])?;
    map_set(interp, this, vec![key, value.clone()])?;
    Ok(value)
}

fn require_new(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(VmErr::Msg(
        "TypeError: Collection constructor requires new".into(),
    ))
}

fn new_map(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    construct(interp, Kind::Map, this, a)
}
fn new_set(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    construct(interp, Kind::Set, this, a)
}
fn new_weak_map(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    construct(interp, Kind::WeakMap, this, a)
}
fn new_weak_set(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    construct(interp, Kind::WeakSet, this, a)
}

// --- Instance methods -------------------------------------------------------

fn size_getter(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let entries = require(&this, "size")?;
    let size = entries.borrow().len();
    Ok(Value::Number(size as f64))
}

fn map_get(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(props) = weak_entries(&this) {
        let storage = props.weak.borrow();
        let WeakStorage::Map(entries) = &*storage else {
            unreachable!()
        };
        return Ok(entries
            .iter()
            .find(|(key, _)| key.matches(a.first().unwrap_or(&Value::Undefined)))
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Undefined));
    }
    let entries = require(&this, "Map.prototype.get")?;
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let Some(index) = position(&entries, &key) else {
        return Ok(Value::Undefined);
    };
    let found = entries.borrow()[index].clone();
    Ok(found.get_prop("1").unwrap_or(Value::Undefined))
}

fn map_set(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(props) = weak_entries(&this) {
        weak_insert(
            interp,
            &props,
            a.first().unwrap_or(&Value::Undefined),
            a.get(1).cloned().unwrap_or(Value::Undefined),
        )?;
        return Ok(this);
    }
    let entries = require(&this, "Map.prototype.set")?;
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let value = a.get(1).cloned().unwrap_or(Value::Undefined);
    match position(&entries, &key) {
        // Re-setting an existing key keeps its original insertion position.
        Some(index) => entries.borrow_mut()[index] = entry(key, value),
        None => {
            if entries.borrow().len() >= crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum collection size exceeded"));
            }
            entries.borrow_mut().push(entry(key, value));
        }
    }
    Ok(this)
}

fn set_add(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(props) = weak_entries(&this) {
        weak_insert(
            interp,
            &props,
            a.first().unwrap_or(&Value::Undefined),
            Value::Undefined,
        )?;
        return Ok(this);
    }
    let entries = require(&this, "Set.prototype.add")?;
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    if position(&entries, &value).is_none() {
        if entries.borrow().len() >= crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum collection size exceeded"));
        }
        entries.borrow_mut().push(entry(value.clone(), value));
    }
    Ok(this)
}

fn collection_has(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(props) = weak_entries(&this) {
        let storage = props.weak.borrow();
        let WeakStorage::Map(entries) = &*storage else {
            unreachable!()
        };
        return Ok(Value::Bool(entries.iter().any(|(key, _)| {
            key.matches(a.first().unwrap_or(&Value::Undefined))
        })));
    }
    let entries = require(&this, "has")?;
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(position(&entries, &key).is_some()))
}

fn collection_delete(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(props) = weak_entries(&this) {
        let mut storage = props.weak.borrow_mut();
        let WeakStorage::Map(entries) = &mut *storage else {
            unreachable!()
        };
        let prior = entries.len();
        entries.retain(|(key, _)| !key.matches(a.first().unwrap_or(&Value::Undefined)));
        return Ok(Value::Bool(entries.len() != prior));
    }
    let entries = require(&this, "delete")?;
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    match position(&entries, &key) {
        Some(index) => {
            entries.borrow_mut().remove(index);
            Ok(Value::Bool(true))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn collection_clear(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    require(&this, "clear")?.borrow_mut().clear();
    Ok(Value::Undefined)
}

/// `forEach(callback, thisArg)`. The callback receives `(value, key,
/// collection)`; for a set the value and the key are the same, as specified.
fn collection_for_each(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let entries = require(&this, "forEach")?;
    let callback = a.first().cloned().unwrap_or(Value::Undefined);
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    // Snapshot: the callback may mutate the collection, and iterating a
    // borrowed `Vec` while guest code runs would panic.
    let snapshot = entries.borrow().clone();
    for item in snapshot {
        let key = item.get_prop("0").unwrap_or(Value::Undefined);
        let value = item.get_prop("1").unwrap_or(Value::Undefined);
        interp.call_this(&callback, receiver.clone(), vec![value, key, this.clone()])?;
    }
    Ok(Value::Undefined)
}

/// Build an array iterator over a projection of the entries.
fn iterate_projection(
    interp: &mut Interpreter,
    this: &Value,
    project: impl Fn(&Value) -> Value,
) -> Result<Value, VmErr> {
    let entries = require(this, "iterator")?;
    let projected: Vec<Value> = entries.borrow().iter().map(project).collect();
    let array = Value::array(projected);
    let iterator = interp.prop(
        &array,
        &Value::String((crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string()).into()),
    )?;
    interp.call_this(&iterator, array, vec![])
}

fn collection_keys(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    iterate_projection(interp, &this, |e| {
        e.get_prop("0").unwrap_or(Value::Undefined)
    })
}

fn collection_values(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    iterate_projection(interp, &this, |e| {
        e.get_prop("1").unwrap_or(Value::Undefined)
    })
}

fn collection_entries(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    iterate_projection(interp, &this, |e| e.clone())
}

/// A collection's kind name and its entries, for callers that need to rebuild
/// it elsewhere — the N-API boundary builds a host `Map`/`Set` from this.
pub fn collection_entries_of(value: &Value) -> Option<(&'static str, Vec<(Value, Value)>)> {
    let kind = kind_of(value)?;
    let entries = entries_of(value)?;
    let pairs = entries
        .borrow()
        .iter()
        .map(|entry| {
            (
                entry.get_prop("0").unwrap_or(Value::Undefined),
                entry.get_prop("1").unwrap_or(Value::Undefined),
            )
        })
        .collect();
    Some((kind.tag(), pairs))
}

/// How a collection renders in the VM's inspection formatter: `Map(2)`,
/// `Set(3)`. `None` for anything that is not one, so the formatter can fall
/// through to its object handling.
pub fn describe_collection(value: &Value) -> Option<String> {
    let kind = kind_of(value)?;
    let entries = entries_of(value)?;
    Some(format!("{}({})", kind.tag(), entries.borrow().len()))
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn final_owner_collection_releases_cached_intrinsic_pins() {
        #[cfg(feature = "napi")]
        let mut context = crate::runtime::OwnerContext::default();
        #[cfg(feature = "napi")]
        let _lease = context.enter();
        let vm = Interpreter::with_builtins();
        {
            let proto = prototype_for(Kind::Map).unwrap();
            proto.set_prop("self".into(), (*proto).clone()).unwrap();
        }
        assert_eq!(PROTOTYPES.with(|protos| protos.borrow().len()), 1);
        drop(vm);
        let stats = crate::heap::collect_after_interpreter_drop();
        assert_eq!(stats.skipped, None);
        assert_eq!(stats.marked, 0);
        assert!(PROTOTYPES.with(|protos| protos.borrow().is_empty()));
        crate::heap::collect();
        assert_eq!(crate::heap::counters().tracked, 0);
    }
}
