//! `Symbol`: unique symbol values, the well-known symbols, and the global
//! registry behind `Symbol.for` / `Symbol.keyFor`.
//!
//! A symbol's identity is an id, not its description: `Symbol('x') !==
//! Symbol('x')`, while a well-known symbol and a registry symbol are the same
//! value every time they are obtained. Ids below
//! [`FIRST_USER_SYMBOL`](crate::value::FIRST_USER_SYMBOL) are reserved for the
//! well-known symbols so that reservation is a compile-time fact.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{BoxedPrimitive, FIRST_USER_SYMBOL, SymbolData, Value};

/// The well-known symbols, in id order starting at 1.
const WELL_KNOWN: &[&str] = &[
    "iterator",
    "asyncIterator",
    "toStringTag",
    "hasInstance",
    "toPrimitive",
    "species",
    "unscopables",
    "isConcatSpreadable",
    "match",
    "matchAll",
    "replace",
    "search",
    "split",
];

thread_local! {
    /// Global symbol registry for `Symbol.for` / `Symbol.keyFor`.
    static SYMBOL_REGISTRY: RefCell<HashMap<String, Rc<SymbolData>>> =
        RefCell::new(HashMap::new());
    /// Source of fresh ids for `Symbol()`.
    static NEXT_ID: Cell<u64> = const { Cell::new(FIRST_USER_SYMBOL) };
}

pub(super) fn install(e: &mut Environment) {
    let constructor = Value::object(vec![]);
    e.set("Symbol", constructor.clone());
    super::make_callable(&constructor, symbol_call, Some(symbol_construct));
    constructor
        .set_prop("for".into(), super::nf("for", symbol_for))
        .expect("Symbol.for");
    constructor
        .set_prop("keyFor".into(), super::nf("keyFor", symbol_key_for))
        .expect("Symbol.keyFor");
    for name in WELL_KNOWN {
        if let Some(symbol) = well_known(name) {
            constructor
                .set_prop((*name).into(), symbol)
                .expect("well-known Symbol property");
        }
    }
    let methods = ["toString", "valueOf"]
        .into_iter()
        .filter_map(|name| symbol_method(name).map(|method| (name, method)))
        .collect();
    let symbol = new_symbol(None);
    let Value::Symbol(primitive) = &symbol else {
        unreachable!("new_symbol returns a symbol")
    };
    super::install_primitive_prototype(e, &constructor, Value::Symbol(primitive.clone()), methods);
}

fn symbol_construct(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(VmErr::Msg("TypeError: Symbol is not a constructor".into()))
}

/// Mint a brand-new symbol. Every call produces a distinct identity.
pub fn new_symbol(description: Option<String>) -> Value {
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    Value::Symbol(Rc::new(SymbolData { id, description }))
}

/// The well-known symbol named `name` (`"iterator"`, `"toStringTag"`, …), or
/// `None` if there is no such symbol.
pub(crate) fn well_known(name: &str) -> Option<Value> {
    let index = WELL_KNOWN.iter().position(|k| *k == name)?;
    Some(Value::Symbol(Rc::new(SymbolData {
        id: index as u64 + 1,
        description: Some(format!("Symbol.{}", name)),
    })))
}

/// Is `value` the well-known `Symbol.iterator`?
pub(crate) fn is_iterator_symbol(value: &Value) -> bool {
    matches!(value, Value::Symbol(s) if s.id == 1)
}

fn symbol_call(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let description = match a.first() {
        None | Some(Value::Undefined) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => Some(interp.vs(v)?),
    };
    Ok(new_symbol(description))
}

/// `Symbol.for(key)`: the one shared symbol for `key`, created on first use.
pub(crate) fn symbol_for(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Undefined) | None => "undefined".to_string(),
        Some(v) => interp.vs(v)?,
    };
    Ok(Value::Symbol(symbol_for_key(&key)))
}

/// Return the shared symbol registry entry for a key. Node-API's
/// `node_api_symbol_for` must use this same registry as guest `Symbol.for`.
pub(crate) fn symbol_for_key(key: &str) -> Rc<SymbolData> {
    if let Some(existing) = SYMBOL_REGISTRY.with(|reg| reg.borrow().get(key).cloned()) {
        return existing;
    }
    let fresh = new_symbol(Some(key.to_owned()));
    let Value::Symbol(data) = &fresh else {
        unreachable!("new_symbol returns a symbol");
    };
    let data = data.clone();
    SYMBOL_REGISTRY.with(|reg| reg.borrow_mut().insert(key.to_owned(), data.clone()));
    data
}

/// `Symbol.keyFor(sym)`: the registry key of a shared symbol, or `undefined`
/// for a symbol that was never registered.
pub(crate) fn symbol_key_for(
    _interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::Symbol(target)) = a.first() else {
        return Ok(Value::Undefined);
    };
    SYMBOL_REGISTRY.with(|reg| {
        Ok(reg
            .borrow()
            .iter()
            .find(|(_, data)| data.id == target.id)
            .map(|(key, _)| Value::String(key.clone()))
            .unwrap_or(Value::Undefined))
    })
}

/// Properties readable on a symbol value itself: `description` and
/// `toString()`.
pub fn symbol_method(key: &str) -> Option<Value> {
    match key {
        "toString" => Some(super::nf("toString", symbol_to_string)),
        "valueOf" => Some(super::nf("valueOf", symbol_value_of)),
        _ => None,
    }
}

fn symbol_to_string(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(symbol_receiver(&this)?.to_display()))
}

fn symbol_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Symbol(symbol_receiver(&this)?))
}

fn symbol_receiver(this: &Value) -> Result<Rc<SymbolData>, VmErr> {
    match this {
        Value::Symbol(symbol) => Ok(symbol.clone()),
        Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
            Some(BoxedPrimitive::Symbol(symbol)) => Ok(symbol.clone()),
            _ => Err(VmErr::Msg(
                "TypeError: Symbol method called on an incompatible receiver".into(),
            )),
        },
        _ => Err(VmErr::Msg(
            "TypeError: Symbol method called on an incompatible receiver".into(),
        )),
    }
}
