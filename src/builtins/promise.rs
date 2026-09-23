//! The `Promise` constructor, its statics, and `Promise.prototype`.
//!
//! Reactions run as microtasks, so `Promise.resolve().then(f); g();` runs `g`
//! first. See `interpreter::promise` for the resolution algorithm itself.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{PromiseInner, PromiseState, PropAttrs, Value};

pub(crate) fn promise_method(name: &str) -> Option<Value> {
    let callable: super::NativeFn = match name {
        "then" => promise_then,
        "catch" => promise_catch,
        "finally" => promise_finally,
        _ => return None,
    };
    Some(super::nf(name, callable))
}

pub(super) fn install(e: &mut Environment) {
    if let Some(p) = e.get("Promise") {
        let statics: &[(&str, super::NativeFn)] = &[
            ("resolve", promise_resolve),
            ("reject", promise_reject),
            ("all", promise_all),
            ("allSettled", promise_all_settled),
            ("race", promise_race),
            ("any", promise_any),
        ];
        for (name, callable) in statics {
            p.set_prop(name.to_string(), super::nf(name, *callable))
                .expect("built-in Promise property");
        }
        super::make_callable(&p, promise_construct, None);

        let object_prototype = e
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let prototype =
            Value::object_with_proto(Vec::new(), object_prototype.map(std::rc::Rc::new));
        prototype
            .set_prop("constructor".into(), p.clone())
            .expect("Promise.prototype constructor");
        for name in ["then", "catch", "finally"] {
            prototype
                .set_prop(
                    name.into(),
                    promise_method(name).expect("listed Promise.prototype method"),
                )
                .expect("Promise.prototype method");
        }
        if let Value::Object { props } = &prototype {
            for name in ["constructor", "then", "catch", "finally"] {
                props.meta.borrow_mut().set_attrs(
                    name,
                    PropAttrs {
                        enumerable: false,
                        ..PropAttrs::default()
                    },
                );
            }
        }
        super::set_builtin_constructor_prototype(e, &p, prototype);
    }
    e.set(
        "queueMicrotask",
        super::nf("queueMicrotask", queue_microtask),
    );
    e.set("setTimeout", super::nf("setTimeout", set_timeout));
    e.set("setInterval", super::nf("setTimeout", set_timeout));
    e.set("clearTimeout", super::nf("clearTimeout", clear_timeout));
    e.set("clearInterval", super::nf("clearTimeout", clear_timeout));
}

fn is_callable(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    )
}

/// `new Promise(executor)`.
///
/// The executor runs *synchronously*, before the constructor returns, and a
/// throw from it rejects the promise.
fn promise_construct(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let executor = a.first().cloned().unwrap_or(Value::Undefined);
    if !is_callable(&executor) {
        return Err(VmErr::Msg(
            "TypeError: Promise resolver is not a function".to_string(),
        ));
    }
    let promise = Value::pending_promise();
    let (resolve, reject) = interp.settle_functions(promise.clone());
    match interp.call_this(&executor, Value::Undefined, vec![resolve, reject]) {
        Ok(_) => {}
        Err(VmErr::Throw(reason)) => interp.reject_promise(&promise, reason),
        Err(other) => return Err(other),
    }
    Ok(Value::Promise(promise))
}

fn promise_resolve(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    // `Promise.resolve` on a promise returns it unchanged.
    if matches!(v, Value::Promise(_)) {
        return Ok(v);
    }
    let promise = Value::pending_promise();
    interp.resolve_promise(&promise, v)?;
    Ok(Value::Promise(promise))
}

fn promise_reject(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::settled_promise(
        PromiseState::Rejected,
        a.first().cloned().unwrap_or(Value::Undefined),
    ))
}

// --- Instance methods -------------------------------------------------------

fn promise_then(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let on_fulfilled = args.first().cloned().unwrap_or(Value::Undefined);
    let on_rejected = args.get(1).cloned().unwrap_or(Value::Undefined);
    interp.register(&this, on_fulfilled, on_rejected, None)
}

fn promise_catch(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let on_rejected = args.first().cloned().unwrap_or(Value::Undefined);
    interp.register(&this, Value::Undefined, on_rejected, None)
}

/// `p.finally(f)`: runs `f` on either settlement and passes the original
/// settlement through, so it is transparent to the chain.
fn promise_finally(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let callback = args.first().cloned().unwrap_or(Value::Undefined);
    let handler = Value::object(vec![
        (FINALLY_SLOT.to_string(), callback),
        (
            crate::interpreter::call::CALL_SLOT.to_string(),
            super::nf("finally", finally_passthrough),
        ),
    ]);
    let rethrow = Value::object(vec![
        (
            FINALLY_SLOT.to_string(),
            handler.get_prop(FINALLY_SLOT).unwrap_or(Value::Undefined),
        ),
        (
            crate::interpreter::call::CALL_SLOT.to_string(),
            super::nf("finally", finally_rethrow),
        ),
    ]);
    interp.register(&this, handler, rethrow, None)
}

const FINALLY_SLOT: &str = "__symbol_finally__";

fn run_finally(interp: &mut Interpreter, this: &Value) -> Result<(), VmErr> {
    let callback = this.get_prop(FINALLY_SLOT).unwrap_or(Value::Undefined);
    if is_callable(&callback) {
        interp.call_this(&callback, Value::Undefined, vec![])?;
    }
    Ok(())
}

fn finally_passthrough(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    run_finally(interp, &this)?;
    Ok(args.into_iter().next().unwrap_or(Value::Undefined))
}

fn finally_rethrow(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    run_finally(interp, &this)?;
    Err(VmErr::Throw(
        args.into_iter().next().unwrap_or(Value::Undefined),
    ))
}

// --- Combinators ------------------------------------------------------------

/// State shared by the reactions of one combinator call.
struct Combinator {
    result: Rc<RefCell<PromiseInner>>,
    slots: Rc<crate::value::ArrayCell>,
    remaining: Rc<std::cell::Cell<usize>>,
}

/// Register `handler` on each input, resolving non-promises through
/// `Promise.resolve` so a plain value participates like a settled promise.
fn each_input(
    interp: &mut Interpreter,
    inputs: &[Value],
    mut register: impl FnMut(&mut Interpreter, usize, &Value) -> Result<(), VmErr>,
) -> Result<(), VmErr> {
    for (index, input) in inputs.iter().enumerate() {
        register(interp, index, input)?;
    }
    Ok(())
}

fn inputs_of(interp: &mut Interpreter, a: &[Value]) -> Result<Vec<Value>, VmErr> {
    match a.first() {
        Some(Value::Array(items)) => Ok(items.borrow().clone()),
        Some(other) => interp.iterate(other),
        None => Ok(Vec::new()),
    }
}

/// Build the `(onFulfilled, onRejected)` pair for one input of a combinator,
/// carrying the shared state and this input's index in hidden slots.
fn reaction_pair(
    state: &Combinator,
    index: usize,
    on_fulfilled: super::NativeFn,
    on_rejected: super::NativeFn,
) -> (Value, Value) {
    let make = |callable: super::NativeFn| {
        Value::object(vec![
            (
                COMBINATOR_SLOT.to_string(),
                Value::Promise(state.result.clone()),
            ),
            (SLOTS_SLOT.to_string(), Value::Array(state.slots.clone())),
            (INDEX_SLOT.to_string(), Value::Number(index as f64)),
            (
                REMAINING_SLOT.to_string(),
                Value::Number(state.remaining.get() as f64),
            ),
            (
                crate::interpreter::call::CALL_SLOT.to_string(),
                super::nf("", callable),
            ),
        ])
    };
    (make(on_fulfilled), make(on_rejected))
}

const COMBINATOR_SLOT: &str = "__symbol_combinator__";
const SLOTS_SLOT: &str = "__symbol_slots__";
const INDEX_SLOT: &str = "__symbol_index__";
const REMAINING_SLOT: &str = "__symbol_remaining__";
/// Counts still-outstanding inputs. Stored in the slots array's last position
/// so every reaction of one call shares it.
fn pending_count(this: &Value) -> Option<Rc<crate::value::ArrayCell>> {
    this.get_prop(SLOTS_SLOT)?.as_array()
}

fn combinator_target(this: &Value) -> Option<Rc<RefCell<PromiseInner>>> {
    this.get_prop(COMBINATOR_SLOT)?.as_promise()
}

fn slot_index(this: &Value) -> usize {
    match this.get_prop(INDEX_SLOT) {
        Some(Value::Number(n)) => n as usize,
        _ => 0,
    }
}

/// Record one input's outcome and, when it was the last one outstanding,
/// settle the combinator's promise with `finish`.
fn record(
    interp: &mut Interpreter,
    this: &Value,
    value: Value,
    finish: impl FnOnce(&mut Interpreter, &Rc<RefCell<PromiseInner>>, Vec<Value>) -> Result<(), VmErr>,
) -> Result<(), VmErr> {
    let (Some(target), Some(slots)) = (combinator_target(this), pending_count(this)) else {
        return Ok(());
    };
    let index = slot_index(this);
    let done = {
        let mut slots = slots.borrow_mut();
        if index < slots.len() {
            slots[index] = value;
        }
        // The tail slot is the outstanding counter.
        let counter = slots.len() - 1;
        let remaining = slots[counter].to_number() - 1.0;
        slots[counter] = Value::Number(remaining);
        remaining <= 0.0
    };
    if done {
        let mut values = slots.borrow().clone();
        values.pop();
        finish(interp, &target, values)?;
    }
    Ok(())
}

/// Allocate the shared state: one slot per input plus a trailing counter.
fn combinator_state(inputs: &[Value]) -> Combinator {
    let mut slots = vec![Value::Undefined; inputs.len()];
    slots.push(Value::Number(inputs.len() as f64));
    Combinator {
        result: Value::pending_promise(),
        slots: Value::array(slots)
            .as_array()
            .expect("Value::array returns an array"),
        remaining: Rc::new(std::cell::Cell::new(inputs.len())),
    }
}

fn promise_all(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let inputs = inputs_of(interp, &a)?;
    let state = combinator_state(&inputs);
    let result = state.result.clone();
    if inputs.is_empty() {
        interp.resolve_promise(&result, Value::array(vec![]))?;
        return Ok(Value::Promise(result));
    }
    each_input(interp, &inputs, |interp, index, input| {
        let (on_fulfilled, on_rejected) = reaction_pair(&state, index, all_fulfil, all_reject);
        interp.register(input, on_fulfilled, on_rejected, None)?;
        Ok(())
    })?;
    Ok(Value::Promise(result))
}

fn all_fulfil(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let value = args.into_iter().next().unwrap_or(Value::Undefined);
    record(interp, &this, value, |interp, target, values| {
        interp.resolve_promise(target, Value::array(values))
    })?;
    Ok(Value::Undefined)
}

/// The first rejection settles `Promise.all` immediately; later outcomes are
/// ignored because a settled promise cannot settle again.
fn all_reject(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(target) = combinator_target(&this) {
        interp.reject_promise(&target, args.into_iter().next().unwrap_or(Value::Undefined));
    }
    Ok(Value::Undefined)
}

fn promise_all_settled(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let inputs = inputs_of(interp, &a)?;
    let state = combinator_state(&inputs);
    let result = state.result.clone();
    if inputs.is_empty() {
        interp.resolve_promise(&result, Value::array(vec![]))?;
        return Ok(Value::Promise(result));
    }
    each_input(interp, &inputs, |interp, index, input| {
        let (on_fulfilled, on_rejected) =
            reaction_pair(&state, index, settled_fulfil, settled_reject);
        interp.register(input, on_fulfilled, on_rejected, None)?;
        Ok(())
    })?;
    Ok(Value::Promise(result))
}

fn settled_record(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    rejected: bool,
) -> Result<Value, VmErr> {
    let value = args.into_iter().next().unwrap_or(Value::Undefined);
    let entry = if rejected {
        Value::object(vec![
            ("status".to_string(), Value::String("rejected".to_string())),
            ("reason".to_string(), value),
        ])
    } else {
        Value::object(vec![
            ("status".to_string(), Value::String("fulfilled".to_string())),
            ("value".to_string(), value),
        ])
    };
    record(interp, &this, entry, |interp, target, values| {
        interp.resolve_promise(target, Value::array(values))
    })?;
    Ok(Value::Undefined)
}

fn settled_fulfil(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    settled_record(interp, this, args, false)
}
fn settled_reject(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    settled_record(interp, this, args, true)
}

fn promise_race(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let inputs = inputs_of(interp, &a)?;
    let state = combinator_state(&inputs);
    let result = state.result.clone();
    each_input(interp, &inputs, |interp, index, input| {
        let (on_fulfilled, on_rejected) = reaction_pair(&state, index, race_fulfil, all_reject);
        interp.register(input, on_fulfilled, on_rejected, None)?;
        Ok(())
    })?;
    Ok(Value::Promise(result))
}

fn race_fulfil(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(target) = combinator_target(&this) {
        let value = args.into_iter().next().unwrap_or(Value::Undefined);
        interp.resolve_promise(&target, value)?;
    }
    Ok(Value::Undefined)
}

/// `Promise.any`: the first fulfilment wins; if every input rejects, the
/// result rejects with an `AggregateError`.
fn promise_any(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let inputs = inputs_of(interp, &a)?;
    let state = combinator_state(&inputs);
    let result = state.result.clone();
    if inputs.is_empty() {
        interp.reject_promise(
            &result,
            Value::Error(crate::value::ErrorData::new(
                "AggregateError",
                "All promises were rejected".to_string(),
            )),
        );
        return Ok(Value::Promise(result));
    }
    each_input(interp, &inputs, |interp, index, input| {
        let (on_fulfilled, on_rejected) = reaction_pair(&state, index, race_fulfil, any_reject);
        interp.register(input, on_fulfilled, on_rejected, None)?;
        Ok(())
    })?;
    Ok(Value::Promise(result))
}

fn any_reject(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let reason = args.into_iter().next().unwrap_or(Value::Undefined);
    record(interp, &this, reason, |interp, target, errors| {
        let aggregate = Value::Error(crate::value::ErrorData::new(
            "AggregateError",
            "All promises were rejected".to_string(),
        ));
        aggregate.set_prop("errors".to_string(), Value::array(errors))?;
        interp.reject_promise(target, aggregate);
        Ok(())
    })?;
    Ok(Value::Undefined)
}

// --- Scheduling globals -----------------------------------------------------

fn queue_microtask(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let callback = a.first().cloned().unwrap_or(Value::Undefined);
    if !is_callable(&callback) {
        return Err(VmErr::Msg(
            "TypeError: queueMicrotask requires a function".to_string(),
        ));
    }
    interp
        .jobs
        .borrow_mut()
        .push_microtask(crate::interpreter::Job::Callback {
            callback,
            args: Vec::new(),
        });
    Ok(Value::Undefined)
}

/// `setTimeout(fn, delay, ...args)`.
///
/// There is no clock in the sandbox: the callback runs after every microtask,
/// ordered against other timers by its delay. That preserves the ordering
/// guest code relies on without letting it observe (or wait on) real time.
fn set_timeout(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let callback = a.first().cloned().unwrap_or(Value::Undefined);
    if !is_callable(&callback) {
        return Ok(Value::Number(0.0));
    }
    let delay = a.get(1).map(|v| v.to_number()).unwrap_or(0.0);
    let args = a.iter().skip(2).cloned().collect();
    let id = interp.jobs.borrow_mut().push_timer(delay, callback, args);
    Ok(Value::Number(id as f64))
}

fn clear_timeout(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let id = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if id > 0.0 {
        interp.jobs.borrow_mut().cancel_timer(id as u64);
    }
    Ok(Value::Undefined)
}
