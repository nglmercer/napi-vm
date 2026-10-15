//! Promise capabilities, species construction, and observable combinator iteration.
use super::{NativeFn, bound_native_method, native_method};
use crate::{
    error::VmErr,
    interpreter::{Environment, Interpreter, is_callable_value, strict_equals},
    value::{PropAttrs, Value},
};

fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Undefined)
}
fn error(message: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {message}"))
}
fn reason(error: VmErr) -> Result<Value, VmErr> {
    match error {
        VmErr::Throw(v) => Ok(v),
        VmErr::RuntimeError(d) => Ok(d.guest_value()),
        VmErr::Msg(m) => Ok(crate::error::error_value_from_msg(&m)),
        other => Err(other),
    }
}
fn state_record(values: Vec<Value>) -> Value {
    Value::object(
        values
            .into_iter()
            .enumerate()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}
fn function(i: &Interpreter, name: &str, length: usize, f: NativeFn, state: Value) -> Value {
    let proto = crate::value::FunctionData::default_function_prototype(&i.persistent_global);
    bound_native_method(name, length, f, proto, state)
}
#[derive(Clone)]
pub(crate) struct Capability {
    pub promise: Value,
    pub resolve: Value,
    pub reject: Value,
}
impl Capability {
    pub(crate) fn new(i: &mut Interpreter, constructor: &Value) -> Result<Self, VmErr> {
        let state = state_record(vec![Value::Undefined, Value::Undefined]);
        let executor = function(i, "", 2, capability_executor, state.clone());
        let promise = i.ctor(constructor, vec![executor])?;
        let resolve = state.get_prop("0").unwrap();
        let reject = state.get_prop("1").unwrap();
        if !is_callable_value(&resolve) || !is_callable_value(&reject) {
            return Err(error("Promise capability functions must be callable"));
        }
        Ok(Self {
            promise,
            resolve,
            reject,
        })
    }
    fn call_reject(&self, i: &mut Interpreter, e: VmErr) -> Result<Value, VmErr> {
        let value = reason(e)?;
        i.call_this(&self.reject, Value::Undefined, vec![value])?;
        Ok(self.promise.clone())
    }
}
fn capability_executor(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if !matches!(this.get_prop("0"), Some(Value::Undefined))
        || !matches!(this.get_prop("1"), Some(Value::Undefined))
    {
        return Err(error("Promise capability executor already called"));
    }
    this.set_prop("0".into(), arg(&a, 0))?;
    this.set_prop("1".into(), arg(&a, 1))?;
    Ok(Value::Undefined)
}

pub(crate) fn promise_method(name: &str) -> Option<Value> {
    let f: NativeFn = match name {
        "then" => promise_then,
        "catch" => promise_catch,
        "finally" => promise_finally,
        _ => return None,
    };
    Some(super::nf(name, f))
}
pub(super) fn install(e: &mut Environment) {
    let Some(p) = e.get("Promise") else { return };
    super::make_callable(&p, super::require_new, Some(promise_construct));
    let fp = e.get("Function").and_then(|v| v.get_prop("prototype"));
    for (name, len, f) in [
        ("resolve", 1, promise_resolve as NativeFn),
        ("reject", 1, promise_reject),
        ("all", 1, promise_all),
        ("allSettled", 1, promise_all_settled),
        ("race", 1, promise_race),
        ("any", 1, promise_any),
        ("withResolvers", 0, promise_with_resolvers),
        ("try", 1, promise_try),
        ("allKeyed", 1, promise_all_keyed),
        ("allSettledKeyed", 1, promise_all_settled_keyed),
    ] {
        p.set_prop(name.into(), native_method(name, len, f, fp.clone()))
            .unwrap();
        p.property_cell().unwrap().meta.borrow_mut().set_attrs(
            name,
            PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    }
    let op = e.get("Object").and_then(|v| v.get_prop("prototype"));
    let proto = Value::object_with_proto(
        vec![("constructor".into(), p.clone())],
        op.map(std::rc::Rc::new),
    );
    for (name, len, f) in [
        ("then", 2, promise_then as NativeFn),
        ("catch", 1, promise_catch),
        ("finally", 1, promise_finally),
    ] {
        proto
            .set_prop(name.into(), native_method(name, len, f, fp.clone()))
            .unwrap();
        proto.property_cell().unwrap().meta.borrow_mut().set_attrs(
            name,
            PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    }
    proto.property_cell().unwrap().meta.borrow_mut().set_attrs(
        "constructor",
        PropAttrs {
            enumerable: false,
            ..Default::default()
        },
    );
    let Value::Symbol(ref tag) = super::well_known("toStringTag").unwrap() else {
        unreachable!()
    };
    let key = crate::interpreter::symbol_slot_key(tag);
    proto
        .set_prop(key.clone(), Value::String("Promise".into()))
        .unwrap();
    proto
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .set_symbol_key(&key, tag.clone());
    proto.property_cell().unwrap().meta.borrow_mut().set_attrs(
        &key,
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );
    super::set_builtin_constructor_prototype(e, &p, proto);
    p.set_prop("length".into(), Value::Number(1.0)).unwrap();
    p.property_cell().unwrap().meta.borrow_mut().set_attrs(
        "length",
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );
    p.set_prop("name".into(), Value::String("Promise".into()))
        .unwrap();
    p.property_cell().unwrap().meta.borrow_mut().set_attrs(
        "name",
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );
    let Value::Symbol(ref species) = super::well_known("species").unwrap() else {
        unreachable!()
    };
    let key = crate::interpreter::symbol_slot_key(species);
    let getter = native_method("get [Symbol.species]", 0, promise_species, fp);
    super::object::define_property(
        &p,
        &key,
        &Value::descriptor_record(vec![
            ("get".into(), getter),
            ("configurable".into(), Value::Bool(true)),
        ]),
    )
    .unwrap();
    p.property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .set_symbol_key(&key, species.clone());
}
fn promise_species(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(this)
}
fn promise_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let executor = arg(&a, 0);
    if !is_callable_value(&executor) {
        return Err(error("Promise executor must be callable"));
    }
    let promise = Value::pending_promise();
    let (resolve, reject) = i.settle_functions(promise.clone());
    if let Err(e) = i.call_this(&executor, Value::Undefined, vec![resolve, reject]) {
        i.reject_promise(&promise, reason(e)?);
    }
    Ok(Value::Promise(promise))
}
pub(crate) fn promise_resolve_with(
    i: &mut Interpreter,
    c: &Value,
    v: Value,
) -> Result<Value, VmErr> {
    if !crate::interpreter::call::is_js_object(c) {
        return Err(error("Promise.resolve receiver must be an object"));
    }
    if v.as_promise().is_some() {
        let constructor = i.get_prop_value_str(&v, "constructor")?;
        if strict_equals(&constructor, c) {
            return Ok(v);
        }
    }
    let cap = Capability::new(i, c)?;
    i.call_this(&cap.resolve, Value::Undefined, vec![v])?;
    Ok(cap.promise)
}
fn promise_resolve(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    promise_resolve_with(i, &this, arg(&a, 0))
}
fn promise_reject(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let cap = Capability::new(i, &this)?;
    i.call_this(&cap.reject, Value::Undefined, vec![arg(&a, 0)])?;
    Ok(cap.promise)
}
fn promise_with_resolvers(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let c = Capability::new(i, &this)?;
    Ok(Value::object(vec![
        ("promise".into(), c.promise),
        ("resolve".into(), c.resolve),
        ("reject".into(), c.reject),
    ]))
}
fn promise_try(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if !crate::interpreter::call::is_js_object(&this) {
        return Err(error("Promise.try receiver must be an object"));
    }
    let result = i.call_this(
        &arg(&a, 0),
        Value::Undefined,
        a.into_iter().skip(1).collect(),
    );
    match result {
        Ok(value) => promise_resolve_with(i, &this, value),
        Err(e) => {
            let cap = Capability::new(i, &this)?;
            cap.call_reject(i, e)
        }
    }
}

fn invoke_then(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let then = i.get_prop_value_str(&this, "then")?;
    i.call_this(&then, this, a)
}
fn promise_then(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if this.as_promise().is_none() {
        return Err(error("Promise.prototype.then requires a Promise"));
    }
    let c = i.species_constructor(&this, "Promise")?;
    let cap = Capability::new(i, &c)?;
    i.register_capability(&this, arg(&a, 0), arg(&a, 1), cap.resolve, cap.reject)?;
    Ok(cap.promise)
}
fn promise_catch(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    invoke_then(i, this, vec![Value::Undefined, arg(&a, 0)])
}
fn promise_finally(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if !crate::interpreter::call::is_js_object(&this) {
        return Err(error("Promise.prototype.finally requires an object"));
    }
    let c = i.species_constructor(&this, "Promise")?;
    let f = arg(&a, 0);
    if !is_callable_value(&f) {
        return invoke_then(i, this, vec![f.clone(), f]);
    }
    let state = state_record(vec![f, c]);
    let yes = function(i, "", 1, finally_fulfil, state.clone());
    let no = function(i, "", 1, finally_reject, state);
    invoke_then(i, this, vec![yes, no])
}
fn finally_run(
    i: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
    rejected: bool,
) -> Result<Value, VmErr> {
    let callback = this.get_prop("0").unwrap();
    let c = this.get_prop("1").unwrap();
    let result = i.call_this(&callback, Value::Undefined, vec![])?;
    let promise = promise_resolve_with(i, &c, result)?;
    let state = state_record(vec![arg(&a, 0), Value::Bool(rejected)]);
    let thunk = function(i, "", 0, finally_thunk, state);
    invoke_then(i, promise, vec![thunk])
}
fn finally_fulfil(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    finally_run(i, t, a, false)
}
fn finally_reject(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    finally_run(i, t, a, true)
}
fn finally_thunk(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let value = this.get_prop("0").unwrap();
    if this.get_prop("1").unwrap().is_truthy() {
        Err(VmErr::Throw(value))
    } else {
        Ok(value)
    }
}

// Combinator state is bound to native closures and traced as ordinary values:
// [resolve, reject, values, remaining, kind, keys]. Remaining starts at one,
// preventing synchronous thenables from completing before iteration finishes.
fn promise_all(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    combine(i, t, a, "all", false)
}
fn promise_all_settled(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    combine(i, t, a, "settled", false)
}
fn promise_any(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    combine(i, t, a, "any", false)
}
fn promise_race(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    combine(i, t, a, "race", false)
}
fn promise_all_keyed(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    combine(i, t, a, "all", true)
}
fn promise_all_settled_keyed(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    combine(i, t, a, "settled", true)
}
fn combine(
    i: &mut Interpreter,
    c: Value,
    a: Vec<Value>,
    kind: &str,
    keyed: bool,
) -> Result<Value, VmErr> {
    let cap = Capability::new(i, &c)?;
    let state = state_record(vec![
        cap.resolve.clone(),
        cap.reject.clone(),
        Value::array(vec![]),
        Value::Number(1.),
        Value::String(kind.into()),
        if keyed {
            Value::array(vec![])
        } else {
            Value::Undefined
        },
    ]);
    let result = (|| {
        let resolve = i.get_prop_value_str(&c, "resolve")?;
        if !is_callable_value(&resolve) {
            return Err(error("Promise resolve must be callable"));
        }
        let source = arg(&a, 0);
        if keyed {
            if !crate::interpreter::call::is_js_object(&source) {
                return Err(error("Keyed promises require an object"));
            }
            for key in i.own_property_keys(&source)? {
                i.consume_loop()?;
                let descriptor = super::object::descriptor_for_key_in(i, &source, &key)?;
                if !matches!(descriptor, Value::Undefined)
                    && descriptor
                        .get_prop("enumerable")
                        .is_some_and(|v| v.is_truthy())
                {
                    let value = i.get_prop_value(&source, &key)?;
                    add_input(i, &state, &c, &resolve, value, Some(key))?;
                }
            }
        } else {
            let iterator = i.iterator_for(&source)?;
            let next = i.get_prop_value_str(&iterator, "next")?;
            loop {
                i.consume_loop()?;
                let step = i.call_this(&next, iterator.clone(), vec![])?;
                if !crate::interpreter::call::is_js_object(&step) {
                    return Err(error("Iterator result must be an object"));
                }
                if i.get_prop_value_str(&step, "done")?.is_truthy() {
                    break;
                }
                let value = i.get_prop_value_str(&step, "value")?;
                if let Err(e) = add_input(i, &state, &c, &resolve, value, None) {
                    i.close_guest_iterator_for_abrupt(&iterator, false, &e)?;
                    return Err(e);
                }
            }
        }
        decrement(i, &state)?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(cap.promise),
        Err(e) => cap.call_reject(i, e),
    }
}
fn add_input(
    i: &mut Interpreter,
    state: &Value,
    c: &Value,
    resolve: &Value,
    value: Value,
    key: Option<Value>,
) -> Result<(), VmErr> {
    let values = state.get_prop("2").unwrap().as_array().unwrap();
    let index = values.borrow().len();
    if index >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum promise input length exceeded",
        ));
    }
    values.borrow_mut().push(Value::Undefined);
    if let Some(key) = key {
        state
            .get_prop("5")
            .unwrap()
            .as_array()
            .unwrap()
            .borrow_mut()
            .push(key);
    }
    let promise = i.call_this(resolve, c.clone(), vec![value])?;
    let kind = state.get_prop("4").unwrap();
    let Value::String(ref kind) = kind else {
        unreachable!()
    };
    let kind = kind.as_str();
    let guard = state_record(vec![Value::Bool(false)]);
    let carrier = Value::array(vec![state.clone(), Value::Number(index as f64), guard]);
    let (yes, no) = match kind {
        "race" => (state.get_prop("0").unwrap(), state.get_prop("1").unwrap()),
        "any" => (
            state.get_prop("0").unwrap(),
            function(i, "", 1, any_reject, carrier),
        ),
        "settled" => (
            function(i, "", 1, settled_fulfil, carrier.clone()),
            function(i, "", 1, settled_reject, carrier),
        ),
        _ => (
            function(i, "", 1, all_fulfil, carrier),
            state.get_prop("1").unwrap(),
        ),
    };
    state.set_prop(
        "3".into(),
        Value::Number(state.get_prop("3").unwrap().to_number() + 1.),
    )?;
    invoke_then(i, promise, vec![yes, no])?;
    Ok(())
}
fn decrement(i: &mut Interpreter, state: &Value) -> Result<(), VmErr> {
    let remaining = state.get_prop("3").unwrap().to_number() - 1.;
    state.set_prop("3".into(), Value::Number(remaining))?;
    if remaining != 0. {
        return Ok(());
    }
    let kind = state.get_prop("4").unwrap();
    let Value::String(ref kind) = kind else {
        unreachable!()
    };
    let kind = kind.as_str();
    if kind == "race" {
        return Ok(());
    }
    let values = state
        .get_prop("2")
        .unwrap()
        .as_array()
        .unwrap()
        .borrow()
        .clone();
    let result = if kind == "any" {
        let ctor = i
            .persistent_global
            .borrow()
            .intrinsic("AggregateError")
            .unwrap();
        i.ctor(
            &ctor,
            vec![
                Value::array(values),
                Value::String("All promises were rejected".into()),
            ],
        )?
    } else if let Some(keys) = state.get_prop("5").unwrap().as_array() {
        let out = Value::object_with_proto(vec![], None);
        out.property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .uses_default_prototype = false;
        for (key, value) in keys.borrow().iter().zip(values) {
            let slot = i.property_key(key)?;
            out.set_prop(slot.clone(), value)?;
            if let Value::Symbol(symbol) = key {
                out.property_cell()
                    .unwrap()
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&slot, symbol.clone());
            }
        }
        out
    } else {
        Value::array(values)
    };
    let callback = state
        .get_prop(if kind == "any" { "1" } else { "0" })
        .unwrap();
    i.call_this(&callback, Value::Undefined, vec![result])?;
    Ok(())
}
fn record(
    i: &mut Interpreter,
    this: Value,
    value: Value,
    settled: Option<bool>,
) -> Result<Value, VmErr> {
    let guard = this.get_prop("2").unwrap();
    if guard.get_prop("0").unwrap().is_truthy() {
        return Ok(Value::Undefined);
    }
    guard.set_prop("0".into(), Value::Bool(true))?;
    let state = this.get_prop("0").unwrap();
    let index = this.get_prop("1").unwrap().to_number() as usize;
    let value = if let Some(rejected) = settled {
        Value::object(vec![
            (
                "status".into(),
                Value::String(if rejected { "rejected" } else { "fulfilled" }.into()),
            ),
            (if rejected { "reason" } else { "value" }.into(), value),
        ])
    } else {
        value
    };
    state
        .get_prop("2")
        .unwrap()
        .as_array()
        .unwrap()
        .borrow_mut()[index] = value;
    decrement(i, &state)?;
    Ok(Value::Undefined)
}
fn all_fulfil(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    record(i, t, arg(&a, 0), None)
}
fn any_reject(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    record(i, t, arg(&a, 0), None)
}
fn settled_fulfil(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    record(i, t, arg(&a, 0), Some(false))
}
fn settled_reject(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    record(i, t, arg(&a, 0), Some(true))
}
