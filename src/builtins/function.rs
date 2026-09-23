//! The `Function` constructor: compiling a function from source at runtime.
//!
//! `new Function('a', 'b', 'return a + b')` parses and builds a function like
//! any other. It is not an escape from the sandbox: the source runs in this
//! interpreter under the same limits as the rest of the program, and — unlike
//! a real `Function` — its scope is the global one, not the caller's, so it
//! cannot reach a local binding it was not passed.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{BoundFunctionData, FunctionData, ObjectCell, Value};

pub(super) fn install(e: &mut Environment) {
    let Some(namespace) = e.get("Function") else {
        return;
    };
    super::make_callable(&namespace, new_function, None);

    let object_prototype = e
        .get("Object")
        .and_then(|object| object.get_prop("prototype"));
    let prototype = Value::Function(Box::new(FunctionData {
        identity: Rc::new(0),
        name: Some(Rc::from("")),
        properties: Rc::new(ObjectCell::new(Vec::new(), object_prototype.map(Rc::new))),
        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
        params: Rc::new(Vec::new()),
        body: Rc::new(Vec::new()),
        closure: None,
        is_arrow: false,
        is_constructor: false,
        is_async: false,
        is_generator: false,
        uses_arguments: false,
        bound: None,
    }));

    prototype
        .set_prop("constructor".to_string(), namespace.clone())
        .expect("Function.prototype constructor");
    if let Value::Function(function) = &prototype {
        function.properties.meta.borrow_mut().set_attrs(
            "constructor",
            crate::value::PropAttrs {
                writable: true,
                enumerable: false,
                configurable: true,
            },
        );
    }
    prototype
        .set_prop("call".to_string(), super::nf("call", function_call))
        .expect("Function.prototype.call");
    prototype
        .set_prop("apply".to_string(), super::nf("apply", function_apply))
        .expect("Function.prototype.apply");
    prototype
        .set_prop("bind".to_string(), super::nf("bind", function_bind))
        .expect("Function.prototype.bind");
    let has_instance =
        crate::builtins::well_known("hasInstance").expect("Symbol.hasInstance is well-known");
    if let Value::Symbol(symbol) = &has_instance {
        let key = crate::interpreter::symbol_slot_key(symbol);
        prototype
            .set_prop(
                key.clone(),
                super::nf("[Symbol.hasInstance]", function_has_instance),
            )
            .expect("Function.prototype[Symbol.hasInstance]");
        if let Value::Function(function) = &prototype {
            function
                .properties
                .meta
                .borrow_mut()
                .set_symbol_key(&key, symbol.clone());
        }
    }
    if let Value::Function(function) = &prototype {
        function.properties.meta.borrow_mut().set_attrs(
            "call",
            crate::value::PropAttrs {
                enumerable: false,
                ..crate::value::PropAttrs::default()
            },
        );
        function.properties.meta.borrow_mut().set_attrs(
            "apply",
            crate::value::PropAttrs {
                enumerable: false,
                ..crate::value::PropAttrs::default()
            },
        );
        function.properties.meta.borrow_mut().set_attrs(
            "bind",
            crate::value::PropAttrs {
                enumerable: false,
                ..crate::value::PropAttrs::default()
            },
        );
        function.properties.meta.borrow_mut().set_attrs(
            &crate::interpreter::symbol_slot_key(match &has_instance {
                Value::Symbol(symbol) => symbol,
                _ => unreachable!("well-known symbol hasInstance"),
            }),
            crate::value::PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
    }

    namespace
        .set_prop("prototype".to_string(), prototype.clone())
        .expect("Function.prototype");
    if let Value::Object { props } = &namespace {
        props.meta.borrow_mut().set_attrs(
            "prototype",
            crate::value::PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
        props.set_proto(Some(Rc::new(prototype)));
    }
}

/// Methods shared by guest and native callable values.
pub(crate) fn function_method(name: &str) -> Option<Value> {
    Some(match name {
        "call" => super::nf("call", function_call),
        "apply" => super::nf("apply", function_apply),
        "bind" => super::nf("bind", function_bind),
        _ => return None,
    })
}

pub(crate) fn is_default_has_instance_method(value: &Value) -> bool {
    let expected: super::NativeFn = function_has_instance;
    matches!(value, Value::NativeFunction { callable, .. }
        if std::ptr::fn_addr_eq(*callable, expected))
}

fn function_bind(
    interp: &mut Interpreter,
    target: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if !is_callable(&target) {
        return Err(VmErr::Msg(
            "TypeError: Function.prototype.bind called on a non-callable value".into(),
        ));
    }

    let bound_this = args.first().cloned().unwrap_or(Value::Undefined);
    let new_arguments: Vec<Value> = args.into_iter().skip(1).collect();
    let target_name = interp.get_prop_value(&target, &Value::String("name".into()))?;
    let target_name = match &target_name {
        Value::String(name) => name.clone(),
        _ => match &target {
            Value::Class(class) => class.name.clone(),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.to_string()
            }
            _ => String::new(),
        },
    };
    let target_length = interp
        .get_prop_value(&target, &Value::String("length".into()))
        .and_then(|length| interp.ecmascript_to_number(&length))?;
    let target_length = if target_length.is_nan() {
        0.0
    } else {
        target_length.trunc()
    };
    let bound_length = (target_length - new_arguments.len() as f64).max(0.0);

    let (bound_target, bound_this, mut bound_arguments) = match &target {
        Value::Function(function) => match &function.bound {
            Some(bound) => (
                bound.target.clone(),
                bound.this_value.clone(),
                bound.arguments.as_ref().clone(),
            ),
            None => (target.clone(), bound_this, Vec::new()),
        },
        _ => (target.clone(), bound_this, Vec::new()),
    };
    if bound_arguments.len().saturating_add(new_arguments.len()) > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum argument count exceeded"));
    }
    bound_arguments.extend(new_arguments);

    let properties = FunctionData::properties_with_default_prototype(&interp.persistent_global);
    properties
        .borrow_mut()
        .push(("name".into(), Value::String(format!("bound {target_name}"))));
    properties.meta.borrow_mut().set_attrs(
        "name",
        crate::value::PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );
    properties
        .borrow_mut()
        .push(("length".into(), Value::Number(bound_length)));
    properties.meta.borrow_mut().set_attrs(
        "length",
        crate::value::PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );

    Ok(Value::Function(Box::new(FunctionData {
        identity: Rc::new(0),
        name: Some(format!("bound {target_name}").into()),
        properties,
        standard_properties_initialized: Rc::new(std::cell::Cell::new(true)),
        params: Rc::new(Vec::new()),
        body: Rc::new(Vec::new()),
        closure: None,
        is_arrow: false,
        is_constructor: is_constructor(&bound_target),
        is_async: false,
        is_generator: false,
        uses_arguments: false,
        bound: Some(Rc::new(BoundFunctionData {
            target: bound_target,
            this_value: bound_this,
            arguments: Rc::new(bound_arguments),
        })),
    })))
}

fn is_callable(value: &Value) -> bool {
    match value {
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => true,
        Value::Proxy(proxy) => is_callable(&proxy.target),
        Value::Object { .. } => {
            crate::interpreter::call::callable_slot(value, crate::interpreter::call::CALL_SLOT)
                .is_some()
        }
        _ => false,
    }
}

fn is_constructor(value: &Value) -> bool {
    match value {
        Value::Function(function) => function.is_constructor,
        Value::HostFunction { .. } | Value::Class(_) => true,
        Value::Proxy(proxy) => is_constructor(&proxy.target),
        Value::Object { .. } => {
            crate::interpreter::call::callable_slot(value, crate::interpreter::call::CONSTRUCT_SLOT)
                .is_some()
                || crate::interpreter::call::callable_slot(
                    value,
                    crate::interpreter::call::CALL_SLOT,
                )
                .is_some()
        }
        _ => false,
    }
}

fn function_has_instance(
    interp: &mut Interpreter,
    target: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if !is_callable(&target) {
        return Ok(Value::Bool(false));
    }
    let object = args.first().cloned().unwrap_or(Value::Undefined);
    if let Value::Function(function) = &target
        && let Some(bound) = &function.bound
    {
        return interp.instance_of(&object, &bound.target);
    }
    interp.ordinary_instance_of(&object, &target)
}

fn function_call(
    interp: &mut Interpreter,
    target: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let receiver = args.first().cloned().unwrap_or(Value::Undefined);
    interp.call_this(&target, receiver, args.into_iter().skip(1).collect())
}

fn function_apply(
    interp: &mut Interpreter,
    target: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let receiver = args.first().cloned().unwrap_or(Value::Undefined);
    let array_like = args.get(1).cloned().unwrap_or(Value::Undefined);
    let call_args = match &array_like {
        Value::Undefined | Value::Null => Vec::new(),
        Value::Array(items) => items.borrow().clone(),
        other => {
            let length_value = interp.get_prop_value(other, &Value::String("length".into()))?;
            let length = interp.ecmascript_to_number(&length_value)?;
            let length = if length.is_nan() || length <= 0.0 {
                0
            } else if !length.is_finite() || length.floor() > crate::value::MAX_ARRAY_LEN as f64 {
                return Err(crate::value::limit_err("Maximum argument count exceeded"));
            } else {
                length.floor() as usize
            };
            let mut values = Vec::with_capacity(length.min(1024));
            for index in 0..length {
                values.push(interp.get_prop_value(other, &Value::String(index.to_string()))?);
            }
            values
        }
    };
    interp.call_this(&target, receiver, call_args)
}

fn new_function(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut params: Vec<String> = Vec::new();
    for value in a.iter().take(a.len().saturating_sub(1)) {
        // One argument may list several parameters: `new Function('a, b', …)`.
        for name in interp.vs(value)?.split(',') {
            let name = name.trim();
            if !name.is_empty() {
                params.push(name.to_string());
            }
        }
    }
    let body_source = match a.last() {
        Some(value) => interp.vs(value)?,
        None => String::new(),
    };

    let tokens = crate::lexer::Lexer::new(&body_source).tokenize_with_spans();
    let mut parser = crate::parser::Parser::new_with_spans(tokens);
    let body = match parser.parse_program() {
        Ok(statements) => statements,
        Err(_) if parser.depth_exceeded => {
            return Err(crate::value::limit_err("Maximum parse depth exceeded"));
        }
        Err(error) => return Err(VmErr::Msg(format!("SyntaxError: {}", error))),
    };

    let uses_arguments = crate::parser::stmts_reference(&body, "arguments");
    Ok(Value::Function(Box::new(FunctionData {
        identity: Rc::new(0),
        name: Some("anonymous".into()),
        properties: FunctionData::properties_with_default_prototype(&interp.persistent_global),
        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
        params: Rc::new(params.iter().map(|p| Rc::from(p.as_str())).collect()),
        body: Rc::new(body),
        // The global scope, not the caller's: a function built from a string
        // must not capture bindings its source never named.
        closure: Some(interp.persistent_global.clone()),
        is_arrow: false,
        is_constructor: true,
        is_async: false,
        is_generator: false,
        uses_arguments,
        bound: None,
    })))
}
