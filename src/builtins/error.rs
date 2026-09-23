//! Constructible `Error` types (`Error`, `TypeError`, `RangeError`,
//! `SyntaxError`, `ReferenceError`). Each is a real class whose instances carry
//! `name` and `message` properties, so `throw new Error("x")` can be caught and
//! inspected as an object (`e.message`, `e.name`).

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{ClassData, ObjectCell, PropAttrs, Value};

const ERROR_TYPES: &[&str] = &[
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
];

pub(super) fn install(e: &mut Environment) {
    let error_class = make_error_class("Error", None);
    let base_prototype = match &error_class {
        Value::Class(class) => Some(class.prototype.clone()),
        _ => unreachable!("Error constructor is a class"),
    };
    e.set("Error", error_class);
    for name in &ERROR_TYPES[1..] {
        e.set(name, make_error_class(name, base_prototype.clone()));
    }
}

fn make_error_class(name: &str, parent_prototype: Option<Rc<Value>>) -> Value {
    let is_base_error = parent_prototype.is_none();
    let constructor = Value::NativeFunction {
        name: name.into(),
        callable: error_ctor,
    };
    let mut properties = vec![
        ("name".to_string(), Value::String(name.to_string())),
        ("message".to_string(), Value::String(String::new())),
        ("stack".to_string(), Value::String(String::new())),
    ];
    if is_base_error {
        properties.push(("toString".to_string(), error_to_string()));
    }
    let prototype = Value::object_with_proto(properties, parent_prototype);
    prototype
        .set_prop("constructor".to_string(), constructor.clone())
        .expect("built-in Error prototype property");
    let statics = Rc::new(ObjectCell::new_with_default_proto(vec![
        ("name".to_string(), Value::String(name.to_string())),
        ("prototype".to_string(), prototype.clone()),
    ]));
    statics.meta.borrow_mut().set_attrs(
        "name",
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );
    statics.meta.borrow_mut().set_attrs(
        "prototype",
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable: false,
        },
    );
    Value::Class(Box::new(ClassData {
        name: name.to_string(),
        constructor: Box::new(constructor),
        prototype: Rc::new(prototype),
        statics,
    }))
}

/// Shared constructor for every error type. The concrete type name is read from
/// the instance's prototype (set per-class above), so one native function
/// serves all five.
/// `Error.prototype.toString`: `"Name: message"`, or just the name when the
/// message is empty.
pub fn error_to_string() -> Value {
    super::nf("toString", error_to_string_impl)
}

fn error_to_string_impl(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let name = match &interp.member(&this, "name")? {
        Value::String(s) => s.clone(),
        _ => "Error".to_string(),
    };
    let message = match &interp.member(&this, "message")? {
        Value::String(s) => s.clone(),
        _ => String::new(),
    };
    Ok(Value::String(if message.is_empty() {
        name
    } else {
        format!("{}: {}", name, message)
    }))
}

fn error_ctor(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let name_prop = this.get_prop("name");
    let name = match &name_prop {
        Some(Value::String(s)) => s.clone(),
        _ => "Error".to_string(),
    };
    let msg = match args.first() {
        None | Some(Value::Undefined) => String::new(),
        Some(v) => interp.vs(v)?,
    };
    // The stack is captured where the error is *constructed*, which is what
    // makes it useful — by the time it is caught, the frames are gone.
    let stack = crate::error::render_stack(&name, &msg, interp.get_stack());
    this.set_prop("message".to_string(), Value::String(msg))?;
    this.set_prop("name".to_string(), Value::String(name))?;
    this.set_prop("stack".to_string(), Value::String(stack))?;
    Ok(Value::Undefined)
}
