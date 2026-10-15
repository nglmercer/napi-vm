//! DisposableStack uses traced, branded records rather than guest properties.
use super::{NativeFn, native_method};
use crate::{
    error::VmErr,
    interpreter::{Environment, Interpreter, is_callable_value},
    value::{PropAttrs, Value},
};
use std::{cell::RefCell, rc::Rc};
#[derive(Default)]
pub(crate) struct Slots {
    disposed: bool,
    asynchronous: bool,
    async_records: Vec<AsyncRecord>,
    records: Vec<(Value, Value, Vec<Value>)>,
}
struct AsyncRecord {
    callback: Value,
    receiver: Value,
    args: Vec<Value>,
    ignore_result: bool,
}
impl Slots {
    pub(crate) fn values(&self) -> Vec<Value> {
        self.records
            .iter()
            .flat_map(|(f, t, a)| {
                std::iter::once(f.clone())
                    .chain(std::iter::once(t.clone()))
                    .chain(a.iter().cloned())
            })
            .chain(self.async_records.iter().flat_map(|r| {
                std::iter::once(r.callback.clone())
                    .chain(std::iter::once(r.receiver.clone()))
                    .chain(r.args.iter().cloned())
            }))
            .collect()
    }
}
fn attrs(target: &Value, key: &str, writable: bool, configurable: bool) {
    target.property_cell().unwrap().meta.borrow_mut().set_attrs(
        key,
        PropAttrs {
            writable,
            enumerable: false,
            configurable,
        },
    );
}
fn symbol(target: &Value, name: &str, value: Value, writable: bool) {
    let Value::Symbol(ref s) = super::well_known(name).unwrap() else {
        unreachable!()
    };
    let key = crate::interpreter::symbol_slot_key(s);
    target.set_prop(key.clone(), value).unwrap();
    let cell = target.property_cell().unwrap();
    cell.meta.borrow_mut().set_symbol_key(&key, s.clone());
    attrs(target, &key, writable, true);
}
pub(super) fn install(e: &mut Environment) {
    install_async(e);
    let fp = e.get("Function").and_then(|v| v.get_prop("prototype"));
    let op = e.get("Object").and_then(|v| v.get_prop("prototype"));
    let constructor = Value::object(vec![]);
    super::make_callable(&constructor, require_new, Some(construct));
    let proto = Value::object_with_proto(vec![], op.map(Rc::new));
    proto
        .set_prop("constructor".into(), constructor.clone())
        .unwrap();
    attrs(&proto, "constructor", true, true);
    for (name, len, f) in [
        ("use", 1, use_resource as NativeFn),
        ("adopt", 2, adopt),
        ("defer", 1, defer),
        ("move", 0, move_stack),
        ("dispose", 0, dispose),
    ] {
        proto
            .set_prop(name.into(), native_method(name, len, f, fp.clone()))
            .unwrap();
        attrs(&proto, name, true, true);
    }
    let getter = native_method("get disposed", 0, disposed, fp.clone());
    super::object::define_property(
        &proto,
        "disposed",
        &Value::descriptor_record(vec![
            ("get".into(), getter),
            ("configurable".into(), Value::Bool(true)),
        ]),
    )
    .expect("disposed accessor");
    symbol(&proto, "dispose", proto.get_prop("dispose").unwrap(), true);
    symbol(
        &proto,
        "toStringTag",
        Value::String("DisposableStack".into()),
        false,
    );
    e.install_intrinsic("%DisposableStackPrototype%", proto.clone());
    super::set_builtin_constructor_prototype(e, &constructor, proto);
    constructor
        .set_prop("name".into(), Value::String("DisposableStack".into()))
        .unwrap();
    attrs(&constructor, "name", false, true);
    constructor
        .set_prop("length".into(), Value::Number(0.))
        .unwrap();
    attrs(&constructor, "length", false, true);
    attrs(&constructor, "prototype", false, false);
    e.set("DisposableStack", constructor);
    e.set_property_attributes(
        "DisposableStack",
        PropAttrs {
            writable: true,
            enumerable: false,
            configurable: true,
        },
    );
    let suppressed = Value::object(vec![]);
    super::make_callable(&suppressed, suppressed_error, Some(suppressed_error));
    let ep = e.get("Error").and_then(|v| v.get_prop("prototype"));
    let sp = Value::object_with_proto(
        vec![
            ("name".into(), Value::String("SuppressedError".into())),
            ("message".into(), Value::String("".into())),
            ("constructor".into(), suppressed.clone()),
        ],
        ep.map(Rc::new),
    );
    for name in ["name", "message", "constructor"] {
        attrs(&sp, name, true, true);
    }
    super::set_builtin_constructor_prototype(e, &suppressed, sp);
    suppressed
        .set_prop("name".into(), Value::String("SuppressedError".into()))
        .unwrap();
    attrs(&suppressed, "name", false, true);
    suppressed
        .set_prop("length".into(), Value::Number(3.))
        .unwrap();
    attrs(&suppressed, "length", false, true);
    attrs(&suppressed, "prototype", false, false);
    suppressed
        .property_cell()
        .unwrap()
        .set_proto(e.get("Error").map(Rc::new));
    e.set("SuppressedError", suppressed);
    e.set_property_attributes(
        "SuppressedError",
        PropAttrs {
            writable: true,
            enumerable: false,
            configurable: true,
        },
    );
}
fn require_new(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(VmErr::Msg("TypeError: DisposableStack requires new".into()))
}
fn allocate(i: &Interpreter) -> Value {
    let proto = i
        .persistent_global
        .borrow()
        .intrinsic("%DisposableStackPrototype%");
    let value = Value::object_with_proto(vec![], proto.map(Rc::new));
    value
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .disposable_stack = Some(Rc::new(RefCell::new(Slots::default())));
    value
}
fn construct(i: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(allocate(i))
}
fn slots(this: &Value) -> Result<Rc<RefCell<Slots>>, VmErr> {
    branded_slots(this, false)
}
fn branded_slots(this: &Value, asynchronous: bool) -> Result<Rc<RefCell<Slots>>, VmErr> {
    let slots = this
        .property_cell()
        .and_then(|p| p.meta.borrow().disposable_stack.clone())
        .ok_or_else(|| VmErr::Msg("TypeError: incompatible disposable stack receiver".into()))?;
    if slots.borrow().asynchronous != asynchronous {
        return Err(VmErr::Msg(
            "TypeError: incompatible disposable stack brand".into(),
        ));
    }
    Ok(slots)
}

fn pending(this: &Value) -> Result<Rc<RefCell<Slots>>, VmErr> {
    let s = slots(this)?;
    if s.borrow().disposed {
        return Err(VmErr::Msg(
            "ReferenceError: DisposableStack is disposed".into(),
        ));
    }
    Ok(s)
}
fn disposed(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(slots(&this)?.borrow().disposed))
}
fn arg(a: &[Value], n: usize) -> Value {
    a.get(n).cloned().unwrap_or(Value::Undefined)
}
fn push(s: &Rc<RefCell<Slots>>, f: Value, t: Value, a: Vec<Value>) -> Result<(), VmErr> {
    let mut s = s.borrow_mut();
    if s.records.len() >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum disposable stack size exceeded",
        ));
    }
    s.records.push((f, t, a));
    Ok(())
}
fn use_resource(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = pending(&this)?;
    let v = arg(&a, 0);
    if !matches!(v, Value::Null | Value::Undefined) {
        if !crate::interpreter::call::is_js_object(&v) {
            return Err(VmErr::Msg("TypeError: resource must be an object".into()));
        }
        let f = i
            .get_method(&v, &super::well_known("dispose").unwrap())?
            .ok_or_else(|| VmErr::Msg("TypeError: resource has no disposer".into()))?;
        push(&s, f, v.clone(), vec![])?;
    }
    Ok(v)
}
fn adopt(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = pending(&this)?;
    let v = arg(&a, 0);
    let f = arg(&a, 1);
    if !is_callable_value(&f) {
        return Err(VmErr::Msg("TypeError: disposer must be callable".into()));
    }
    push(&s, f, Value::Undefined, vec![v.clone()])?;
    Ok(v)
}
fn defer(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = pending(&this)?;
    let f = arg(&a, 0);
    if !is_callable_value(&f) {
        return Err(VmErr::Msg("TypeError: disposer must be callable".into()));
    }
    push(&s, f, Value::Undefined, vec![])?;
    Ok(Value::Undefined)
}
fn move_stack(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = pending(&this)?;
    let out = allocate(i);
    let records = std::mem::take(&mut s.borrow_mut().records);
    slots(&out)?.borrow_mut().records = records;
    s.borrow_mut().disposed = true;
    Ok(out)
}
fn suppressed_error(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let proto = i
        .persistent_global
        .borrow()
        .intrinsic("SuppressedError")
        .and_then(|c| c.get_prop("prototype"));
    let out = Value::object_with_proto(vec![], proto.map(Rc::new));
    out.property_cell().unwrap().meta.borrow_mut().error_object = true;
    if !matches!(a.get(2), None | Some(Value::Undefined)) {
        out.set_prop(
            "message".into(),
            Value::String(i.ecmascript_to_string(&a[2])?),
        )?;
    }
    out.set_prop("error".into(), arg(&a, 0))?;
    out.set_prop("suppressed".into(), arg(&a, 1))?;
    for key in ["error", "suppressed", "message"] {
        attrs(&out, key, true, true);
    }
    Ok(out)
}
fn dispose(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slots(&this)?;
    if s.borrow().disposed {
        return Ok(Value::Undefined);
    }
    s.borrow_mut().disposed = true;
    let records = std::mem::take(&mut s.borrow_mut().records);
    let mut error = None;
    for (f, t, a) in records.into_iter().rev() {
        i.consume_loop()?;
        if let Err(e) = i.call_this(&f, t, a) {
            let reason = match e {
                VmErr::Throw(v) => v,
                VmErr::Msg(m) => crate::error::error_value_from_msg(&m),
                other => return Err(other),
            };
            error = Some(match error {
                None => reason,
                Some(previous) => suppressed_error(i, Value::Undefined, vec![reason, previous])?,
            });
        }
    }
    match error {
        Some(v) => Err(VmErr::Throw(v)),
        None => Ok(Value::Undefined),
    }
}

fn install_async(e: &mut Environment) {
    let constructor = Value::object(vec![]);
    super::make_callable(&constructor, super::require_new, Some(async_construct));
    let fp = e.get("Function").and_then(|v| v.get_prop("prototype"));
    let op = e.get("Object").and_then(|v| v.get_prop("prototype"));
    let proto = Value::object_with_proto(
        vec![("constructor".into(), constructor.clone())],
        op.map(Rc::new),
    );
    attrs(&proto, "constructor", true, true);
    for (name, len, f) in [
        ("use", 1, async_use as NativeFn),
        ("adopt", 2, async_adopt),
        ("defer", 1, async_defer),
        ("move", 0, async_move),
        ("disposeAsync", 0, async_dispose),
    ] {
        proto
            .set_prop(name.into(), native_method(name, len, f, fp.clone()))
            .unwrap();
        attrs(&proto, name, true, true);
    }
    let getter = native_method("get disposed", 0, async_disposed, fp);
    super::object::define_property(
        &proto,
        "disposed",
        &Value::descriptor_record(vec![
            ("get".into(), getter),
            ("configurable".into(), Value::Bool(true)),
        ]),
    )
    .unwrap();
    symbol(
        &proto,
        "asyncDispose",
        proto.get_prop("disposeAsync").unwrap(),
        true,
    );
    symbol(
        &proto,
        "toStringTag",
        Value::String("AsyncDisposableStack".into()),
        false,
    );
    e.install_intrinsic("%AsyncDisposableStackPrototype%", proto.clone());
    super::set_builtin_constructor_prototype(e, &constructor, proto);
    constructor
        .set_prop("length".into(), Value::Number(0.))
        .unwrap();
    attrs(&constructor, "length", false, true);
    constructor
        .set_prop("name".into(), Value::String("AsyncDisposableStack".into()))
        .unwrap();
    attrs(&constructor, "name", false, true);
    attrs(&constructor, "prototype", false, false);
    e.set("AsyncDisposableStack", constructor);
    e.set_property_attributes(
        "AsyncDisposableStack",
        PropAttrs {
            writable: true,
            enumerable: false,
            configurable: true,
        },
    );
}
fn async_allocate(i: &Interpreter) -> Value {
    let proto = i
        .persistent_global
        .borrow()
        .intrinsic("%AsyncDisposableStackPrototype%");
    let value = Value::object_with_proto(vec![], proto.map(Rc::new));
    value
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .disposable_stack = Some(Rc::new(RefCell::new(Slots {
        asynchronous: true,
        ..Default::default()
    })));
    value
}
fn async_construct(i: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(async_allocate(i))
}
fn async_pending(this: &Value) -> Result<Rc<RefCell<Slots>>, VmErr> {
    let slots = branded_slots(this, true)?;
    if slots.borrow().disposed {
        return Err(VmErr::Msg(
            "ReferenceError: AsyncDisposableStack is disposed".into(),
        ));
    }
    Ok(slots)
}
fn async_disposed(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(branded_slots(&this, true)?.borrow().disposed))
}
fn async_push(slots: &Rc<RefCell<Slots>>, record: AsyncRecord) -> Result<(), VmErr> {
    let mut slots = slots.borrow_mut();
    if slots.async_records.len() >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum disposable stack size exceeded",
        ));
    }
    slots.async_records.push(record);
    Ok(())
}
fn async_use(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let slots = async_pending(&this)?;
    let value = arg(&a, 0);
    let (callback, ignore_result) = if matches!(value, Value::Null | Value::Undefined) {
        (Value::Undefined, true)
    } else {
        if !crate::interpreter::call::is_js_object(&value) {
            return Err(VmErr::Msg("TypeError: resource must be an object".into()));
        }
        match i.get_method(&value, &super::well_known("asyncDispose").unwrap())? {
            Some(f) => (f, false),
            None => {
                let f = i
                    .get_method(&value, &super::well_known("dispose").unwrap())?
                    .ok_or_else(|| VmErr::Msg("TypeError: resource has no disposer".into()))?;
                (f, true)
            }
        }
    };
    async_push(
        &slots,
        AsyncRecord {
            callback,
            receiver: value.clone(),
            args: vec![],
            ignore_result,
        },
    )?;
    Ok(value)
}
fn async_adopt(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let slots = async_pending(&this)?;
    let value = arg(&a, 0);
    let callback = arg(&a, 1);
    if !is_callable_value(&callback) {
        return Err(VmErr::Msg("TypeError: disposer must be callable".into()));
    }
    async_push(
        &slots,
        AsyncRecord {
            callback,
            receiver: Value::Undefined,
            args: vec![value.clone()],
            ignore_result: false,
        },
    )?;
    Ok(value)
}
fn async_defer(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let slots = async_pending(&this)?;
    let callback = arg(&a, 0);
    if !is_callable_value(&callback) {
        return Err(VmErr::Msg("TypeError: disposer must be callable".into()));
    }
    async_push(
        &slots,
        AsyncRecord {
            callback,
            receiver: Value::Undefined,
            args: vec![],
            ignore_result: false,
        },
    )?;
    Ok(Value::Undefined)
}
fn async_move(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let slots = async_pending(&this)?;
    let out = async_allocate(i);
    branded_slots(&out, true)?.borrow_mut().async_records =
        std::mem::take(&mut slots.borrow_mut().async_records);
    slots.borrow_mut().disposed = true;
    Ok(out)
}
fn async_reason(error: VmErr) -> Result<Value, VmErr> {
    match error {
        VmErr::Throw(value) => Ok(value),
        VmErr::RuntimeError(data) => Ok(data.guest_value()),
        VmErr::Msg(message) => Ok(crate::error::error_value_from_msg(&message)),
        other => Err(other),
    }
}
fn async_dispose(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let promise = Value::Promise(Value::pending_promise());
    let slots = match branded_slots(&this, true) {
        Ok(slots) => slots,
        Err(e) => {
            i.reject_promise(&promise.as_promise().unwrap(), async_reason(e)?);
            return Ok(promise);
        }
    };
    if slots.borrow().disposed {
        i.resolve_promise(&promise.as_promise().unwrap(), Value::Undefined)?;
        return Ok(promise);
    }
    slots.borrow_mut().disposed = true;
    let state = Value::object(vec![
        ("stack".into(), this),
        ("promise".into(), promise.clone()),
        ("has_error".into(), Value::Bool(false)),
        ("error".into(), Value::Undefined),
    ]);
    async_advance(i, &state)?;
    Ok(promise)
}
fn async_error(i: &mut Interpreter, state: &Value, error: Value) -> Result<(), VmErr> {
    let error = if state.get_prop("has_error").unwrap().is_truthy() {
        suppressed_error(
            i,
            Value::Undefined,
            vec![error, state.get_prop("error").unwrap()],
        )?
    } else {
        error
    };
    state.set_prop("error".into(), error)?;
    state.set_prop("has_error".into(), Value::Bool(true))?;
    Ok(())
}
fn async_advance(i: &mut Interpreter, state: &Value) -> Result<(), VmErr> {
    loop {
        i.consume_loop()?;
        let stack = state.get_prop("stack").unwrap();
        let slots = branded_slots(&stack, true)?;
        let record = slots.borrow_mut().async_records.pop();
        let Some(record) = record else {
            let promise = state.get_prop("promise").unwrap().as_promise().unwrap();
            if state.get_prop("has_error").unwrap().is_truthy() {
                i.reject_promise(&promise, state.get_prop("error").unwrap());
            } else {
                i.resolve_promise(&promise, Value::Undefined)?;
            }
            return Ok(());
        };
        let result = if matches!(record.callback, Value::Undefined) {
            Ok(Value::Undefined)
        } else {
            i.call_this(&record.callback, record.receiver, record.args)
        };
        let value = match result {
            Ok(v) => {
                if record.ignore_result {
                    Value::Undefined
                } else {
                    v
                }
            }
            Err(e) => {
                async_error(i, state, async_reason(e)?)?;
                continue;
            }
        };
        let promise = match i.promise_resolve_intrinsic(value) {
            Ok(p) => p,
            Err(e) => {
                async_error(i, state, async_reason(e)?)?;
                continue;
            }
        };
        let proto = crate::value::FunctionData::default_function_prototype(&i.persistent_global);
        let yes = super::bound_native_method("", 1, async_resume, proto.clone(), state.clone());
        let no = super::bound_native_method("", 1, async_reject_resume, proto, state.clone());
        i.register(&promise, yes, no, None)?;
        return Ok(());
    }
}
fn async_resume(i: &mut Interpreter, state: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    async_advance(i, &state)?;
    Ok(Value::Undefined)
}
fn async_reject_resume(i: &mut Interpreter, state: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    async_error(i, &state, arg(&a, 0))?;
    async_advance(i, &state)?;
    Ok(Value::Undefined)
}

/// A lexical resource record is owned by its environment, not by guest bindings.
#[derive(Clone)]
pub(crate) struct Resource {
    method: Value,
    value: Value,
    asynchronous: bool,
    ignore_result: bool,
}
impl Resource {
    pub(crate) fn values(&self) -> [Value; 2] {
        [self.method.clone(), self.value.clone()]
    }
}
pub(crate) fn acquire(
    i: &mut Interpreter,
    value: Value,
    asynchronous: bool,
) -> Result<Option<Resource>, VmErr> {
    if matches!(value, Value::Null | Value::Undefined) {
        return Ok(asynchronous.then_some(Resource {
            method: Value::Undefined,
            value: Value::Undefined,
            asynchronous,
            ignore_result: true,
        }));
    }
    if !crate::interpreter::call::is_js_object(&value) {
        return Err(VmErr::Msg("TypeError: resource must be an object".into()));
    }
    let (method, ignore_result) = if asynchronous {
        match i.get_method(&value, &super::well_known("asyncDispose").unwrap())? {
            Some(method) => (method, false),
            None => {
                let method = i
                    .get_method(&value, &super::well_known("dispose").unwrap())?
                    .ok_or_else(|| VmErr::Msg("TypeError: resource has no disposer".into()))?;
                (method, true)
            }
        }
    } else {
        let method = i
            .get_method(&value, &super::well_known("dispose").unwrap())?
            .ok_or_else(|| VmErr::Msg("TypeError: resource has no disposer".into()))?;
        (method, false)
    };
    Ok(Some(Resource {
        method,
        value,
        asynchronous,
        ignore_result,
    }))
}
pub(crate) fn dispose_scope(
    i: &mut Interpreter,
    records: Vec<Resource>,
    mut completion: Result<Value, VmErr>,
) -> Result<Value, VmErr> {
    if matches!(&completion,Err(e) if e.is_abandon()) {
        return completion;
    }
    for record in records.into_iter().rev() {
        i.consume_loop()?;
        let result = (|| {
            let value = if matches!(record.method, Value::Undefined) {
                Value::Undefined
            } else {
                i.call_this(&record.method, record.value, vec![])?
            };
            if record.asynchronous {
                i.perform_await(if record.ignore_result {
                    Value::Undefined
                } else {
                    value
                })?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            completion = match completion {
                Err(previous @ (VmErr::Throw(_) | VmErr::RuntimeError(_) | VmErr::Msg(_))) => {
                    let new = async_reason(error)?;
                    let previous = async_reason(previous)?;
                    Err(VmErr::Throw(suppressed_error(
                        i,
                        Value::Undefined,
                        vec![new, previous],
                    )?))
                }
                _ => Err(error),
            };
        }
    }
    completion
}
