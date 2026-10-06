use super::*;
fn is_callable(v: &Value) -> bool {
    napi_vm_core::interpreter::is_callable_value(v)
}
pub fn install_timers(e: &mut Environment) {
    e.set(
        "queueMicrotask",
        super::nf("queueMicrotask", queue_microtask),
    );
    e.set("setTimeout", super::nf("setTimeout", set_timeout));
    e.set("setInterval", super::nf("setInterval", set_interval));
    e.set("clearTimeout", super::nf("clearTimeout", clear_timeout));
    e.set("clearInterval", super::nf("clearTimeout", clear_timeout));
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
    interp.jobs.borrow().check_timer_capacity()?;
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

fn set_interval(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let callback = a.first().cloned().unwrap_or(Value::Undefined);
    if !is_callable(&callback) {
        return Err(VmErr::Msg(
            "TypeError: timer callback must be callable".into(),
        ));
    }
    let delay = a.get(1).map(|v| v.to_number()).unwrap_or(0.0);
    let args = a.iter().skip(2).cloned().collect();
    interp.jobs.borrow().check_timer_capacity()?;
    let id = interp
        .jobs
        .borrow_mut()
        .push_interval(delay, callback, args);
    Ok(Value::Number(id as f64))
}
