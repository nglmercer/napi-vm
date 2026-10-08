//! The intrinsic async adapter for synchronous iterator records.
use std::rc::Rc;

use super::Interpreter;
use crate::error::{VmErr, error_value_from_msg};
use crate::value::{PromiseState, Value};

#[derive(Clone)]
pub(crate) enum Slots {
    Iterator {
        iterator: Value,
        next: Value,
    },
    Reaction {
        iterator: Value,
        done: bool,
        close_on_rejection: bool,
    },
}

impl Slots {
    pub(crate) fn values(&self) -> Vec<Value> {
        match self {
            Self::Iterator { iterator, next } => vec![iterator.clone(), next.clone()],
            Self::Reaction { iterator, .. } => vec![iterator.clone()],
        }
    }
}

pub(crate) fn create(vm: &mut Interpreter, iterator: Value) -> Result<Value, VmErr> {
    let next = vm.member(&iterator, "next")?;
    let prototype = vm
        .persistent_global
        .borrow()
        .intrinsic("%AsyncFromSyncIteratorPrototype%");
    let adapter = Value::object_with_proto(vec![], prototype.map(Rc::new));
    adapter
        .property_cell()
        .expect("adapter object")
        .meta
        .borrow_mut()
        .async_from_sync = Some(Slots::Iterator { iterator, next });
    Ok(adapter)
}

#[derive(Clone, Copy)]
enum Operation {
    Next,
    Return,
    Throw,
}

pub(crate) fn next(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    perform(vm, this, args, Operation::Next)
}
pub(crate) fn return_(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    perform(vm, this, args, Operation::Return)
}
pub(crate) fn throw(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    perform(vm, this, args, Operation::Throw)
}

fn reason(error: VmErr) -> Value {
    match error {
        VmErr::Throw(reason) => reason,
        VmErr::RuntimeError(error) => error.guest_value(),
        error => error_value_from_msg(&error.to_string()),
    }
}

fn perform(
    vm: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    operation: Operation,
) -> Result<Value, VmErr> {
    let result = (|| {
        let slots = this
            .property_cell()
            .and_then(|properties| properties.meta.borrow().async_from_sync.clone());
        let Some(Slots::Iterator { iterator, next }) = slots else {
            return Err(VmErr::Msg(
                "TypeError: Async-from-sync method requires an iterator receiver".into(),
            ));
        };
        let method = match operation {
            Operation::Next => next,
            Operation::Return => match vm.get_method(&iterator, &Value::String("return".into()))? {
                Some(method) => method,
                None => {
                    let value = args.first().cloned().unwrap_or(Value::Undefined);
                    let promise = Value::pending_promise();
                    vm.resolve_promise(&promise, super::call::iter_result(value, true))?;
                    return Ok(Value::Promise(promise));
                }
            },
            Operation::Throw => match vm.get_method(&iterator, &Value::String("throw".into()))? {
                Some(method) => method,
                None => {
                    vm.close_guest_iterator(&iterator, false)?;
                    return Err(VmErr::Msg(
                        "TypeError: Synchronous iterator has no throw method".into(),
                    ));
                }
            },
        };
        // Preserve whether the caller supplied an argument. IteratorNext on
        // a for-await loop calls next with no arguments.
        let result = vm.call_this(
            &method,
            iterator.clone(),
            args.into_iter().take(1).collect(),
        )?;
        if !super::call::is_js_object(&result) {
            return Err(VmErr::Msg(
                "TypeError: Iterator result must be an object".into(),
            ));
        }
        // AsyncFromSyncIteratorContinuation reads both properties before
        // PromiseResolve, including value on a completed iterator.
        let done = vm.member(&result, "done")?.is_truthy();
        let value = vm.member(&result, "value")?;
        let awaited = match vm.promise_resolve_intrinsic(value) {
            Ok(promise) => promise,
            Err(error) => {
                if !done && matches!(operation, Operation::Next) {
                    let _ = vm.close_guest_iterator(&iterator, false);
                }
                return Err(error);
            }
        };
        let make = |callable: crate::builtins::NativeFn| {
            let callback = Value::object(vec![(
                super::call::CALL_SLOT.into(),
                Value::NativeFunction {
                    name: "".into(),
                    callable,
                },
            )]);
            callback
                .property_cell()
                .expect("reaction object")
                .meta
                .borrow_mut()
                .async_from_sync = Some(Slots::Reaction {
                iterator: iterator.clone(),
                done,
                close_on_rejection: matches!(operation, Operation::Next),
            });
            callback
        };
        vm.register(&awaited, make(fulfilled), make(rejected), None)
    })();
    Ok(match result {
        Ok(promise) => promise,
        Err(error) => Value::settled_promise(PromiseState::Rejected, reason(error)),
    })
}

fn fulfilled(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let slots = this
        .property_cell()
        .and_then(|properties| properties.meta.borrow().async_from_sync.clone());
    let Some(Slots::Reaction { done, .. }) = slots else {
        return Err(VmErr::Msg("Invalid async-from-sync reaction".into()));
    };
    Ok(super::call::iter_result(
        args.first().cloned().unwrap_or(Value::Undefined),
        done,
    ))
}

fn rejected(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let reason = args.first().cloned().unwrap_or(Value::Undefined);
    let slots = this
        .property_cell()
        .and_then(|properties| properties.meta.borrow().async_from_sync.clone());
    if let Some(Slots::Reaction {
        iterator,
        done: false,
        close_on_rejection: true,
    }) = slots
    {
        // IteratorClose with a throw completion preserves the original error.
        let _ = vm.close_guest_iterator(&iterator, false);
    }
    Err(VmErr::Throw(reason))
}
