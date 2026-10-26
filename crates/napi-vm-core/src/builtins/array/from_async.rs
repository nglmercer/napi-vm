//! Sequential async collection with traced continuation records.
use crate::builtins::{NativeFn, bound_native_method};
use crate::{error::VmErr, interpreter::Interpreter, value::Value};

fn get(s: &Value, key: &str) -> Value {
    s.get_prop(key).expect("fromAsync state")
}
fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn reason(error: VmErr) -> Result<Value, VmErr> {
    match error {
        VmErr::Throw(v) => Ok(v),
        VmErr::RuntimeError(d) => Ok(d.guest_value()),
        VmErr::Msg(m) => Ok(crate::error::error_value_from_msg(&m)),
        other => Err(other),
    }
}
fn callback(i: &Interpreter, f: NativeFn, state: Value) -> Value {
    bound_native_method(
        "",
        1,
        f,
        crate::value::FunctionData::default_function_prototype(&i.persistent_global),
        state,
    )
}
pub(super) fn from_async(i: &mut Interpreter, c: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let promise = Value::Promise(Value::pending_promise());
    let state = record(vec![
        ("promise", promise.clone()),
        ("source", args.first().cloned().unwrap_or(Value::Undefined)),
        ("mapper", args.get(1).cloned().unwrap_or(Value::Undefined)),
        ("this", args.get(2).cloned().unwrap_or(Value::Undefined)),
        ("index", Value::Number(0.)),
        ("iterator", Value::Undefined),
        ("close", Value::Bool(false)),
    ]);
    if let Err(e) = initialize(i, &state, c) {
        fail(i, &state, e)?;
    }
    Ok(promise)
}
fn initialize(i: &mut Interpreter, s: &Value, c: Value) -> Result<(), VmErr> {
    let source = get(s, "source");
    let mapper = get(s, "mapper");
    if !matches!(mapper, Value::Undefined) && !crate::interpreter::is_callable_value(&mapper) {
        return Err(VmErr::Msg(
            "TypeError: Array.fromAsync mapper must be callable".into(),
        ));
    }
    let asynchronous = i.get_method(
        &source,
        &crate::builtins::well_known("asyncIterator").unwrap(),
    )?;
    let synchronous = if asynchronous.is_none() {
        i.get_method(&source, &crate::builtins::well_known("iterator").unwrap())?
    } else {
        None
    };
    if let Some(method) = asynchronous.as_ref().or(synchronous.as_ref()) {
        let result = super::array_result_create(i, &c, 0.0, true)?;
        s.set_prop("result".into(), result)?;
        let mut iterator = i.iterator_from_method(&source, method)?;
        if asynchronous.is_none() {
            iterator = crate::interpreter::async_from_sync::create(i, iterator)?;
        }
        let next = i.get_prop_value_str(&iterator, "next")?;
        s.set_prop("iterator".into(), iterator)?;
        s.set_prop("next".into(), next)?;
        s.set_prop("length".into(), Value::Undefined)?;
    } else {
        let source = crate::builtins::object::to_object_receiver(&source)?;
        let length = super::array_like_length_number(i, &source)?;
        let result = if crate::builtins::is_constructor(&c) {
            i.ctor(&c, vec![Value::Number(length)])?
        } else {
            super::array_result_create(i, &c, length, false)?
        };
        s.set_prop("source".into(), source)?;
        s.set_prop("result".into(), result)?;
        s.set_prop("length".into(), Value::Number(length))?;
    }
    advance(i, s)
}
fn await_value(
    i: &mut Interpreter,
    s: &Value,
    value: Value,
    fulfilled: NativeFn,
) -> Result<(), VmErr> {
    let promise = i.promise_resolve_intrinsic(value)?;
    i.register(
        &promise,
        callback(i, fulfilled, s.clone()),
        callback(i, rejected, s.clone()),
        None,
    )?;
    Ok(())
}
fn advance(i: &mut Interpreter, s: &Value) -> Result<(), VmErr> {
    i.consume_loop()?;
    let index = get(s, "index").to_number() as usize;
    let iterator = get(s, "iterator");
    if matches!(iterator, Value::Undefined) {
        if index >= get(s, "length").to_number() as usize {
            return finish(i, s);
        }
        let value = i.get_prop_value_str(&get(s, "source"), &index.to_string())?;
        await_value(i, s, value, value_ready)
    } else {
        s.set_prop("close".into(), Value::Bool(false))?;
        let value = i.call_this(&get(s, "next"), iterator, vec![])?;
        await_value(i, s, value, step_ready)
    }
}
fn finish(i: &mut Interpreter, s: &Value) -> Result<(), VmErr> {
    s.set_prop("close".into(), Value::Bool(false))?;
    let result = get(s, "result");
    i.set_member_str_or_throw(&result, "length", get(s, "index"))?;
    i.resolve_promise(&get(s, "promise").as_promise().unwrap(), result)
}
fn step_ready(i: &mut Interpreter, s: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let result = (|| {
        let step = a.first().cloned().unwrap_or(Value::Undefined);
        if !crate::interpreter::call::is_js_object(&step) {
            return Err(VmErr::Msg(
                "TypeError: async iterator result must be an object".into(),
            ));
        }
        if i.get_prop_value_str(&step, "done")?.is_truthy() {
            return finish(i, &s);
        }
        let value = i.get_prop_value_str(&step, "value")?;
        s.set_prop("close".into(), Value::Bool(true))?;
        // An actual async iterator controls whether its value is awaited.
        // The async-from-sync adapter has already awaited a sync value once.
        map_value(i, &s, value)
    })();
    complete_callback(i, &s, result)
}
fn value_ready(i: &mut Interpreter, s: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let result = map_value(i, &s, a.first().cloned().unwrap_or(Value::Undefined));
    complete_callback(i, &s, result)
}
fn map_value(i: &mut Interpreter, s: &Value, value: Value) -> Result<(), VmErr> {
    let index = get(s, "index").to_number() as usize;
    if index >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let mapper = get(s, "mapper");
    if matches!(mapper, Value::Undefined) {
        store_value(i, s, value)
    } else {
        let value = i.call_this(&mapper, get(s, "this"), vec![value, get(s, "index")])?;
        await_value(i, s, value, mapped_ready)
    }
}
fn mapped_ready(i: &mut Interpreter, s: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let result = store_value(i, &s, a.first().cloned().unwrap_or(Value::Undefined));
    complete_callback(i, &s, result)
}
fn store_value(i: &mut Interpreter, s: &Value, value: Value) -> Result<(), VmErr> {
    let index = get(s, "index").to_number() as usize;
    super::create_array_result_element(i, &get(s, "result"), index, value)?;
    s.set_prop("index".into(), Value::Number((index + 1) as f64))?;
    advance(i, s)
}
fn complete_callback(
    i: &mut Interpreter,
    s: &Value,
    result: Result<(), VmErr>,
) -> Result<Value, VmErr> {
    if let Err(e) = result {
        fail(i, s, e)?;
    }
    Ok(Value::Undefined)
}
fn rejected(i: &mut Interpreter, s: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    fail(
        i,
        &s,
        VmErr::Throw(a.first().cloned().unwrap_or(Value::Undefined)),
    )?;
    Ok(Value::Undefined)
}
fn fail(i: &mut Interpreter, s: &Value, e: VmErr) -> Result<(), VmErr> {
    let reason = reason(e)?;
    let iterator = get(s, "iterator");
    if get(s, "close").is_truthy() && !matches!(iterator, Value::Undefined) {
        s.set_prop("close".into(), Value::Bool(false))?;
        s.set_prop("error".into(), reason.clone())?;
        let closing = (|| {
            let method = i.get_method(&iterator, &Value::String("return".into()))?;
            let Some(method) = method else {
                return Ok(None);
            };
            i.call_this(&method, iterator, vec![]).map(Some)
        })();
        if let Ok(Some(value)) = closing
            && let Ok(promise) = i.promise_resolve_intrinsic(value)
        {
            let handler = callback(i, closed, s.clone());
            i.register(&promise, handler.clone(), handler, None)?;
            return Ok(());
        }
    }
    i.reject_promise(&get(s, "promise").as_promise().unwrap(), reason);
    Ok(())
}
fn closed(i: &mut Interpreter, s: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    i.reject_promise(&get(&s, "promise").as_promise().unwrap(), get(&s, "error"));
    Ok(Value::Undefined)
}
