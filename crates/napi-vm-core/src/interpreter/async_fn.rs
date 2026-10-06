//! Async functions: bodies that genuinely suspend at `await`.
//!
//! An async call runs its body on a coroutine — the same stack-switching
//! machinery generators use, on the calling thread — and returns a promise
//! immediately. Reaching an `await` suspends the body and hands the awaited
//! value back to the driver, which registers a reaction on it; when that
//! settles, a microtask resumes the body where it left off.
//!
//! That is what makes the ordering right. With an eager implementation
//! `async function f() { log(1); await 0; log(3); } f(); log(2);` prints
//! 1, 3, 2; suspending prints 1, 2, 3, because the continuation after `await`
//! is a microtask like any other.

#[cfg(stackful_coroutines)]
use std::cell::RefCell;
#[cfg(stackful_coroutines)]
use std::rc::Rc;

use super::Interpreter;
#[cfg(stackful_coroutines)]
use super::{Environment, Realm};
use crate::error::VmErr;
#[cfg(stackful_coroutines)]
use crate::value::{GenOutcome, GenResume, PromiseInner};
use crate::value::{PromiseState, Value};

/// The suspended body of one in-flight async call.
#[cfg(stackful_coroutines)]
pub struct AsyncTask {
    execution: Rc<super::scheduler::ExecutionState>,
    counted: bool,
    coroutine: Option<crate::value::GenCoroutine>,
    /// The promise the call returned, settled when the body finishes.
    result: Rc<RefCell<PromiseInner>>,
}

#[cfg(stackful_coroutines)]
impl std::fmt::Debug for AsyncTask {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "AsyncTask")
    }
}

#[cfg(stackful_coroutines)]
impl AsyncTask {
    /// Whether this task's suspended coroutine holds values the tracer
    /// cannot see. The result promise is traced in its own right.
    pub(crate) fn suspends_values(&self) -> bool {
        self.coroutine.is_some()
    }

    /// The promise the call returned, for the iterative `Drop` of `Value`.
    pub(crate) fn result_promise(&self) -> Rc<RefCell<PromiseInner>> {
        self.result.clone()
    }
}

impl Interpreter {
    /// Evaluate `await value`.
    ///
    /// Inside an async body this suspends; at the top level (where there is no
    /// body to suspend) it drains the microtask queue until the promise
    /// settles, which is the closest a synchronous entry point can get.
    pub(crate) fn perform_await(&mut self, value: Value) -> Result<Value, VmErr> {
        // An async host call parks the VM thread until the host answers; that
        // is a different mechanism (a real blocking wait) and stays as it was.
        if let Value::HostPending { id } = value {
            let bridge = self.host.clone().ok_or_else(|| {
                VmErr::Msg("cannot await host promise: no bridge attached".to_string())
            })?;
            return bridge.await_host(id);
        }

        #[cfg(stackful_coroutines)]
        if let Some(yielder) = self.await_yielder.as_ref() {
            // Suspend, handing the awaited value to the driver. It resumes us
            // with the settled value, or with a throw for a rejection.
            return match yielder.suspend(value) {
                GenResume::Next(v) => Ok(v.unwrap_or(Value::Undefined)),
                GenResume::Throw(reason) => Err(VmErr::Throw(reason)),
                // The task was abandoned; unwind the body so `finally` runs.
                GenResume::Return => Err(VmErr::Ret(Value::Undefined)),
                // Dropped while suspended: unwind with no guest handlers.
                GenResume::Abandon => Err(VmErr::Abandon),
            };
        }

        let value = if value.as_promise().is_some() {
            value
        } else {
            let wrapper = Value::pending_promise();
            self.resolve_promise(&wrapper, value)?;
            Value::Promise(wrapper)
        };
        self.await_synchronously(value)
    }

    /// `await` at the top level, where there is no body to suspend: run the
    /// event loop until the promise settles, then unwrap it.
    ///
    /// The queue is drained even for a non-promise, because `await` always
    /// yields to the microtask queue. Skipping that would let code after a
    /// top-level `await 0` observe reactions that a real engine has already
    /// run. If a promise remains pending with no timer or host work able to
    /// settle it, fail clearly instead of returning its placeholder value.
    fn await_synchronously(&mut self, value: Value) -> Result<Value, VmErr> {
        let Some(promise) = value.as_promise() else {
            self.drain_microtasks()?;
            return Ok(value);
        };
        loop {
            self.drain_microtasks()?;
            if promise.borrow().state != PromiseState::Pending {
                break;
            }
            // Microtasks are exhausted and it is still pending; run the next
            // timer before checking whether its own host bridge has work.
            if self.run_await_timer()? {
                continue;
            }
            #[cfg(not(target_arch = "wasm32"))]
            let timer_wait = self.jobs.borrow().timer_wait();
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(wait) = timer_wait {
                self.run_await_event(wait)?;
                continue;
            }
            if self.jobs.borrow().next_deadline().is_some() {
                return Err(VmErr::Msg("pending Promise requires host-driven timer progress; advance the virtual clock or poll from the browser host before awaiting again".into()));
            }
            let has_pending_host_work = self
                .host
                .as_ref()
                .is_some_and(|bridge| bridge.has_pending_host_work(&promise));
            if has_pending_host_work {
                self.run_await_event(std::time::Duration::from_millis(10))?;
                continue;
            }
            break;
        }
        let inner = promise.borrow();
        match inner.state {
            PromiseState::Pending => Err(VmErr::Msg(
                "cannot synchronously await a pending Promise: no VM or host event can settle it"
                    .into(),
            )),
            PromiseState::Rejected => Err(VmErr::Throw(inner.value.clone())),
            PromiseState::Fulfilled => Ok(inner.value.clone()),
        }
    }
}

/// Initial stack size for an async body's coroutine. Recursive guest calls
/// can grow additional guarded segments; `MAX_CALL_DEPTH` bounds total depth.
#[cfg(stackful_coroutines)]
const ASYNC_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Start an async function body on its own stack and return the promise for
/// its completion.
#[cfg(stackful_coroutines)]
pub(crate) fn spawn_async(
    interp: &mut Interpreter,
    body: Rc<Vec<crate::parser::Statement>>,
    frame: super::Env,
    gen_depth: u32,
) -> Result<Value, VmErr> {
    spawn_body(interp, body, frame, gen_depth, false)
}
#[cfg(stackful_coroutines)]
pub(crate) fn spawn_module(
    interp: &mut Interpreter,
    body: Rc<Vec<crate::parser::Statement>>,
    frame: super::Env,
) -> Result<Value, VmErr> {
    spawn_body(interp, body, frame, interp.gen_depth + 1, true)
}
#[cfg(stackful_coroutines)]
fn spawn_body(
    interp: &mut Interpreter,
    body: Rc<Vec<crate::parser::Statement>>,
    frame: super::Env,
    gen_depth: u32,
    linked: bool,
) -> Result<Value, VmErr> {
    use corosensei::Coroutine;
    use corosensei::stack::DefaultStack;

    let result = Value::pending_promise();
    let realm = Realm::of(interp);
    let builtins_env = interp.global.borrow().parent_env();
    let persistent_global = interp.persistent_global.clone();
    let host = interp.host.clone();
    let module_context = interp.cur_mod.clone();
    let compiled = if linked {
        crate::bytecode::compiler::compile_linked_program(&body)
    } else {
        crate::bytecode::compile_program(&body)
    };
    let bytecode = match compiled {
        Ok(code) => {
            crate::bytecode::verify_module(&code)
                .map_err(|error| VmErr::Msg(format!("internal error: {error}")))?;
            Some(code)
        }
        Err(_) => None,
    };

    let Ok(stack) = DefaultStack::new(ASYNC_STACK_SIZE) else {
        // Stack allocation failed. Reject rather than abort the process.
        interp.reject_promise(
            &result,
            Value::Error(crate::value::ErrorData::new(
                "RangeError",
                "Could not allocate an async call stack".to_string(),
            )),
        );
        return Ok(Value::Promise(result));
    };

    let coroutine = Coroutine::with_stack(stack, move |yielder, _first| {
        let mut body_interp = match builtins_env {
            Some(builtins) => {
                let mut i = Interpreter::new();
                i.global = Rc::new(RefCell::new(Environment::child(builtins)));
                i
            }
            None => Interpreter::with_builtins(),
        };
        body_interp.persistent_global = persistent_global;
        body_interp.host = host;
        body_interp.cur_mod = module_context;
        // One event loop and one module registry across every stack: a promise
        // settled in here must schedule reactions the outer drain will run,
        // and an `import` here must resolve against the same modules.
        realm.install(&mut body_interp);
        body_interp.gen_depth = gen_depth;
        // SAFETY: identical to the generator case — the yielder is borrowed
        // from this coroutine's own frame, `body_interp` is created here and
        // dropped when the closure returns, and `GenYielder` is `!Send`.
        body_interp.await_yielder = Some(unsafe { crate::value::GenYielder::new(yielder) });
        body_interp.global = frame;

        let outcome = match &bytecode {
            Some(code) => body_interp.run_bytecode_module(code),
            None if linked => body_interp.run(&body),
            None => body_interp.run_program_body(&body),
        };
        match outcome {
            Ok(_) => GenOutcome::Returned(Value::Undefined),
            Err(VmErr::Ret(v)) => GenOutcome::Returned(v),
            Err(VmErr::Throw(v)) => GenOutcome::Threw(v),
            // Abandoned while suspended: the initiating `Drop` consumes this.
            Err(VmErr::Abandon) => GenOutcome::Abandon,
            Err(VmErr::Msg(m)) => GenOutcome::Failed(m),
            Err(VmErr::RuntimeError(e)) => GenOutcome::Failed(e.message.clone()),
            Err(e @ (VmErr::Break(_) | VmErr::Continue(_))) => GenOutcome::Failed(format!("{}", e)),
        }
    });

    interp
        .execution
        .continuations
        .set(interp.execution.continuations.get() + 1);
    let task = crate::heap::tracked(Rc::new(RefCell::new(AsyncTask {
        execution: interp.execution.clone(),
        counted: true,
        coroutine: Some(coroutine),
        result: result.clone(),
    })));
    step(interp, &task, GenResume::Next(None))?;
    Ok(Value::Promise(result))
}

/// Resume an async body once and act on where it stopped.
#[cfg(stackful_coroutines)]
fn step(
    interp: &mut Interpreter,
    task: &Rc<RefCell<AsyncTask>>,
    resume: GenResume,
) -> Result<(), VmErr> {
    // Take the coroutine out for the duration of the resume: holding a borrow
    // across guest code would panic if the body reached back in.
    let Some(mut coroutine) = task.borrow_mut().coroutine.take() else {
        return Ok(());
    };
    let outcome = coroutine.resume(resume);
    if matches!(&outcome, corosensei::CoroutineResult::Return(_)) {
        let mut task = task.borrow_mut();
        task.execution
            .continuations
            .set(task.execution.continuations.get() - 1);
        task.counted = false;
    }
    let result = task.borrow().result.clone();
    match outcome {
        // Suspended at an `await`: continue when the awaited value settles.
        corosensei::CoroutineResult::Yield(awaited) => {
            task.borrow_mut().coroutine = Some(coroutine);
            let (on_fulfilled, on_rejected) = resume_handlers(task);
            // A non-promise is awaited through `Promise.resolve`, so the
            // continuation is still a microtask rather than running inline.
            let bridged = match awaited.as_promise() {
                Some(_) => awaited,
                None => {
                    let wrapper = Value::pending_promise();
                    interp.resolve_promise(&wrapper, awaited)?;
                    Value::Promise(wrapper)
                }
            };
            if let Some(awaited_promise) = bridged.as_promise()
                && awaited_promise.borrow().state == PromiseState::Pending
                && awaited_promise.borrow().external_pending
            {
                result.borrow_mut().external_pending = true;
            }
            interp.register(&bridged, on_fulfilled, on_rejected, None)?;
            Ok(())
        }
        corosensei::CoroutineResult::Return(GenOutcome::Returned(value)) => {
            interp.resolve_promise(&result, value)
        }
        // A throw becomes the rejection reason, unchanged: `catch (e)` on the
        // other side must see the error object the body threw.
        corosensei::CoroutineResult::Return(GenOutcome::Threw(value)) => {
            interp.reject_promise(&result, value);
            Ok(())
        }
        // An internal failure (a limit, an escaped signal) is not a guest
        // value; it becomes an `Error` describing what went wrong.
        corosensei::CoroutineResult::Return(GenOutcome::Failed(message)) => {
            interp.reject_promise(
                &result,
                Value::Error(crate::value::ErrorData::new("Error", message)),
            );
            Ok(())
        }
        // Unreachable: only an abandon-resume produces this, and the
        // initiating `Drop` consumes it. Leave the promise pending rather
        // than surfacing internals.
        corosensei::CoroutineResult::Return(GenOutcome::Abandon) => Ok(()),
    }
}

#[cfg(stackful_coroutines)]
impl Drop for AsyncTask {
    /// Tear down a task dropped while suspended at an `await`, without the
    /// platform unwinder — the same abandon path generators use (see
    /// [`crate::value::force_abandon`]). Dropping the coroutine directly
    /// would force-unwind across stacks, which faults on Windows.
    fn drop(&mut self) {
        if self.counted {
            self.execution
                .continuations
                .set(self.execution.continuations.get() - 1);
        }
        if let Some(coroutine) = self.coroutine.take()
            && !coroutine.done()
        {
            crate::value::force_abandon(coroutine);
        }
    }
}

/// Hidden slot carrying the suspended task through the reaction functions.
#[cfg(stackful_coroutines)]
const TASK_SLOT: &str = "__symbol_async_task__";

/// Build the `(onFulfilled, onRejected)` pair that resumes `task`.
///
/// A native function is a bare pointer, so the task travels in a hidden
/// property of a callable object rather than in a capture.
#[cfg(stackful_coroutines)]
fn resume_handlers(task: &Rc<RefCell<AsyncTask>>) -> (Value, Value) {
    let handle = Value::AsyncTask(task.clone());
    let make = |callable: fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>| {
        Value::object(vec![
            (TASK_SLOT.to_string(), handle.clone()),
            (
                super::call::CALL_SLOT.to_string(),
                Value::NativeFunction {
                    name: "".into(),
                    callable,
                },
            ),
        ])
    };
    (make(resume_with_value), make(resume_with_throw))
}

#[cfg(stackful_coroutines)]
fn task_of(this: &Value) -> Option<Rc<RefCell<AsyncTask>>> {
    match &this.get_prop(TASK_SLOT)? {
        Value::AsyncTask(task) => Some(task.clone()),
        _ => None,
    }
}

#[cfg(stackful_coroutines)]
fn resume_with_value(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if let Some(task) = task_of(&this) {
        step(
            interp,
            &task,
            GenResume::Next(Some(args.into_iter().next().unwrap_or(Value::Undefined))),
        )?;
    }
    Ok(Value::Undefined)
}

#[cfg(stackful_coroutines)]
fn resume_with_throw(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if let Some(task) = task_of(&this) {
        step(
            interp,
            &task,
            GenResume::Throw(args.into_iter().next().unwrap_or(Value::Undefined)),
        )?;
    }
    Ok(Value::Undefined)
}
