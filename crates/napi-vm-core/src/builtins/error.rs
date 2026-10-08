//! Callable and constructible Error intrinsics with realm-owned prototypes.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{PropAttrs, Value};

pub(super) fn install(e: &mut Environment) {
    let object_prototype = e
        .get("Object")
        .and_then(|object| object.get_prop("prototype"));
    let function_prototype = e
        .get("Function")
        .and_then(|function| function.get_prop("prototype"));
    let mut base_prototype = None;
    for (name, callable) in [
        ("Error", error_constructor as super::NativeFn),
        ("TypeError", type_error_constructor as super::NativeFn),
        ("RangeError", range_error_constructor as super::NativeFn),
        ("SyntaxError", syntax_error_constructor as super::NativeFn),
        (
            "ReferenceError",
            reference_error_constructor as super::NativeFn,
        ),
        ("EvalError", eval_error_constructor as super::NativeFn),
        ("URIError", uri_error_constructor as super::NativeFn),
        (
            "AggregateError",
            aggregate_error_constructor as super::NativeFn,
        ),
    ] {
        let constructor = Value::object(vec![
            ("name".into(), Value::String(name.into())),
            (
                "length".into(),
                Value::Number(if name == "AggregateError" { 2.0 } else { 1.0 }),
            ),
        ]);
        super::make_callable(&constructor, callable, None);
        let parent = base_prototype.clone().or_else(|| object_prototype.clone());
        let prototype = Value::object_with_proto(
            vec![
                ("constructor".into(), constructor.clone()),
                ("name".into(), Value::String(name.into())),
                ("message".into(), Value::String(crate::JsString::default())),
                ("stack".into(), Value::String(crate::JsString::default())),
            ],
            parent.map(Rc::new),
        );
        if name == "Error" {
            constructor
                .set_prop(
                    "isError".into(),
                    super::native_method("isError", 1, error_is_error, function_prototype.clone()),
                )
                .expect("Error.isError");
            constructor
                .property_cell()
                .expect("Error constructor")
                .meta
                .borrow_mut()
                .set_attrs(
                    "isError",
                    PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            prototype
                .set_prop(
                    "toString".into(),
                    super::native_method(
                        "toString",
                        0,
                        error_to_string_impl,
                        function_prototype.clone(),
                    ),
                )
                .expect("Error.prototype.toString");
            base_prototype = Some(prototype.clone());
        }
        if let Value::Object { props } = &prototype {
            for key in props.borrow().iter().map(|(key, _)| key) {
                props.meta.borrow_mut().set_attrs(
                    key,
                    PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            }
        }
        if let Value::Object { props } = &constructor {
            for key in ["name", "length"] {
                props.meta.borrow_mut().set_attrs(
                    key,
                    PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        super::set_builtin_constructor_prototype(e, &constructor, prototype);
        if name != "Error"
            && let Value::Object { props } = &constructor
        {
            props.set_proto(e.get("Error").map(Rc::new));
        }
        e.set(name, constructor);
    }
}

/// Error.prototype.toString applies ToString in observable name/message order.
pub fn error_to_string() -> Value {
    super::nf("toString", error_to_string_impl)
}

fn error_to_string_impl(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    if !crate::interpreter::call::is_js_object(&this) {
        return Err(VmErr::Msg(
            "TypeError: Error.prototype.toString requires an object".into(),
        ));
    }
    let name = match interp.member(&this, "name")? {
        Value::Undefined => crate::JsString::from("Error"),
        value => interp.ecmascript_to_string(&value)?,
    };
    let message = match interp.member(&this, "message")? {
        Value::Undefined => crate::JsString::default(),
        value => interp.ecmascript_to_string(&value)?,
    };
    Ok(Value::String(if message.is_empty() {
        name
    } else if name.is_empty() {
        message
    } else {
        name.concat(&crate::JsString::from(": ")).concat(&message)
    }))
}

macro_rules! error_constructors {
    ($($function:ident => $name:literal),* $(,)?) => {
        $(fn $function(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
            construct_error(interp, $name, args)
        })*
    };
}

error_constructors! {
    error_constructor => "Error",
    type_error_constructor => "TypeError",
    range_error_constructor => "RangeError",
    syntax_error_constructor => "SyntaxError",
    reference_error_constructor => "ReferenceError",
    eval_error_constructor => "EvalError",
    uri_error_constructor => "URIError",
}

fn construct_error(interp: &mut Interpreter, name: &str, args: Vec<Value>) -> Result<Value, VmErr> {
    let prototype = interp
        .persistent_global
        .borrow()
        .intrinsic(name)
        .and_then(|constructor| constructor.get_prop("prototype"));
    let error = Value::object_with_proto(vec![], prototype.map(Rc::new));
    error
        .property_cell()
        .expect("Error instance")
        .meta
        .borrow_mut()
        .error_object = true;
    let message = match args.first() {
        None | Some(Value::Undefined) => crate::JsString::default(),
        Some(value) => {
            let message = interp.ecmascript_to_string(value)?;
            error.set_prop("message".into(), Value::String(message.clone()))?;
            message
        }
    };
    if let Some(options) = args
        .get(1)
        .filter(|value| crate::interpreter::call::is_js_object(value))
    {
        let key = Value::String("cause".into());
        if interp.has_property(options, &key)? {
            let cause = interp.get_prop_value(options, &key)?;
            error.set_prop("cause".into(), cause)?;
        }
    }
    let tail = crate::error::render_stack("", "", interp.get_stack());
    let stack = crate::JsString::from(name)
        .concat(&crate::JsString::from(": "))
        .concat(&message)
        .concat(&crate::JsString::from(tail));
    error.set_prop("stack".into(), Value::String(stack))?;
    if let Value::Object { props } = &error {
        for key in ["message", "cause", "stack"] {
            if props.borrow().iter().any(|(property, _)| property == key) {
                props.meta.borrow_mut().set_attrs(
                    key,
                    PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            }
        }
    }
    Ok(error)
}

fn error_is_error(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let error = args.first().is_some_and(Value::is_error_object);
    Ok(Value::Bool(error))
}

fn aggregate_error_constructor(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    // Message and cause precede iteration, using the same Error construction
    // semantics and the active realm's AggregateError prototype.
    let error = construct_error(
        interp,
        "AggregateError",
        args.iter().skip(1).cloned().collect(),
    )?;
    let errors = args.first().cloned().unwrap_or(Value::Undefined);
    let list = interp.drain_iterable(&errors)?;
    error.set_prop("errors".into(), Value::checked_array(list)?)?;
    error
        .property_cell()
        .expect("AggregateError instance")
        .meta
        .borrow_mut()
        .set_attrs(
            "errors",
            PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    Ok(error)
}
