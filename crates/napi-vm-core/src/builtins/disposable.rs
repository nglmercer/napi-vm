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
    records: Vec<(Value, Value, Vec<Value>)>,
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
    this.property_cell()
        .and_then(|p| p.meta.borrow().disposable_stack.clone())
        .ok_or_else(|| VmErr::Msg("TypeError: incompatible DisposableStack receiver".into()))
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
