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
use crate::value::{ArrayCell, Value};

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
        super::make_callable(&namespace, kind.constructor(), None);
        // Each VM gets its own prototype object: instances inherit from the
        // namespace's live `.prototype` (identity matters for `instanceof`),
        // and the `constructor` back-link must point at this VM's namespace,
        // not a cross-VM shared object.
        let proto = build_prototype(kind).expect("collection prototype");
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
        namespace
            .set_prop("prototype".to_string(), proto)
            .expect("collection prototype link");
        if let Value::Object { props } = &namespace {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Map,
    Set,
    WeakMap,
    WeakSet,
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
    match &this.get_prop(KIND_SLOT)? {
        Value::String(tag) => match tag.as_str() {
            "Map" => Some(Kind::Map),
            "Set" => Some(Kind::Set),
            "WeakMap" => Some(Kind::WeakMap),
            "WeakSet" => Some(Kind::WeakSet),
            _ => None,
        },
        _ => None,
    }
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
    static PROTOTYPES: RefCell<Vec<(&'static str, Rc<Value>)>> = const { RefCell::new(Vec::new()) };
}

/// A fresh prototype object: methods, the `size` getter and the iterator.
/// The caller decides whether it is cached or installed per-VM.
fn build_prototype(kind: Kind) -> Result<Value, VmErr> {
    let proto = Value::object(vec![]);
    for (name, callable) in methods(kind) {
        proto.set_prop(name.to_string(), super::nf(name, callable))?;
    }
    // `size` is a getter, so it tracks mutation instead of freezing at
    // construction time. The `get ` name prefix is what the property resolver
    // recognizes as an accessor.
    if !matches!(kind, Kind::WeakMap | Kind::WeakSet) {
        proto.set_prop("size".to_string(), super::nf("get size", size_getter))?;
        proto.set_prop(
            crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string(),
            super::nf("[Symbol.iterator]", collection_iterator),
        )?;
    }
    Ok(proto)
}

fn prototype_for(kind: Kind) -> Result<Rc<Value>, VmErr> {
    if let Some(existing) = PROTOTYPES.with(|protos| {
        protos
            .borrow()
            .iter()
            .find(|(tag, _)| *tag == kind.tag())
            .map(|(_, proto)| proto.clone())
    }) {
        return Ok(existing);
    }
    let proto = Rc::new(build_prototype(kind)?);
    PROTOTYPES.with(|protos| protos.borrow_mut().push((kind.tag(), proto.clone())));
    Ok(proto)
}

/// The `[[Prototype]]` for a new instance: the namespace's live `.prototype`
/// (OrdinaryCreateFromConstructor), falling back to the intrinsic default
/// when a guest replaced it with a non-object.
fn instance_proto(interp: &mut Interpreter, kind: Kind) -> Result<Rc<Value>, VmErr> {
    // The global lookup ends before `member` runs: the member read can
    // execute guest getters, which need the interpreter mutably.
    let namespace = interp.global.borrow().get(kind.tag());
    if let Some(namespace) = namespace
        && let Ok(proto) = interp.member(&namespace, "prototype")
        && matches!(proto, Value::Object { .. })
    {
        return Ok(Rc::new(proto));
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
    let namespace = interp.global.borrow().get(kind.tag());
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
            target.set_prop(KIND_SLOT.to_string(), Value::String(kind.tag().to_string()))?;
            target
        }
        None => Value::object_with_proto(
            vec![
                (ENTRIES_SLOT.to_string(), Value::array(vec![])),
                (KIND_SLOT.to_string(), Value::String(kind.tag().to_string())),
            ],
            Some(instance_proto(interp, kind)?),
        ),
    };

    if let Some(source) = args.first()
        && !matches!(source, Value::Undefined | Value::Null)
    {
        let items = interp.iterate(source)?;
        let entries = require(&collection, kind.tag())?;
        for item in items {
            let (key, value) = if kind.keyed() {
                (interp.member(&item, "0")?, interp.member(&item, "1")?)
            } else {
                (item.clone(), item)
            };
            if position(&entries, &key).is_none() {
                if entries.borrow().len() >= crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum collection size exceeded"));
                }
                entries.borrow_mut().push(entry(key, value));
            }
        }
    }
    Ok(collection)
}

fn methods(kind: Kind) -> Vec<(&'static str, super::NativeFn)> {
    let mut out: Vec<(&'static str, super::NativeFn)> =
        vec![("has", collection_has), ("delete", collection_delete)];
    if kind.keyed() {
        out.push(("get", map_get));
        out.push(("set", map_set));
    } else {
        out.push(("add", set_add));
    }
    if !matches!(kind, Kind::WeakMap | Kind::WeakSet) {
        out.push(("clear", collection_clear));
        out.push(("forEach", collection_for_each));
        out.push(("keys", collection_keys));
        out.push(("values", collection_values));
        out.push(("entries", collection_entries));
    }
    out
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
    let entries = require(&this, "Map.prototype.get")?;
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let Some(index) = position(&entries, &key) else {
        return Ok(Value::Undefined);
    };
    let found = entries.borrow()[index].clone();
    Ok(found.get_prop("1").unwrap_or(Value::Undefined))
}

fn map_set(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
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

fn set_add(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
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
    let entries = require(&this, "has")?;
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(position(&entries, &key).is_some()))
}

fn collection_delete(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
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
        &Value::String(crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string()),
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

/// A collection's default iterator: entries for a map, values for a set.
fn collection_iterator(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    match kind_of(&this) {
        Some(kind) if kind.keyed() => collection_entries(interp, this, vec![]),
        _ => collection_values(interp, this, vec![]),
    }
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
