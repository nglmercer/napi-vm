mod array;
mod bigint;
mod collections;
mod date;
mod error;
mod function;
pub(crate) mod json;
mod math;
mod number;
pub(crate) mod object;
mod promise;
mod proxy;
mod reflect;
pub(crate) mod regexp;
mod string;
mod symbol;
mod typedarray;
mod weak;
pub(crate) use symbol::is_registered as symbol_is_registered;

pub(crate) use function::function_method;
pub(crate) use function::is_constructor;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use function::is_default_has_instance_method;
pub(crate) use promise::promise_method;

pub use array::array_method;
pub use bigint::bigint_method;
pub use collections::{collection_entries_of, collection_tag, describe_collection};
pub use date::{date_member, iso_string};
pub use error::error_to_string;
pub use number::number_method;
pub(crate) use regexp::compile as compile_regex;
pub use regexp::regexp_member;
pub use string::string_method;
pub use symbol::new_symbol;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use symbol::symbol_for_key;
pub use symbol::symbol_method;
pub(crate) use symbol::{is_iterator_symbol, symbol_for, symbol_key_for, well_known};
pub use typedarray::{
    array_buffer_member, data_view_member, read_element, shared_array_buffer_member, typed_member,
    write_element,
};

use crate::error::VmErr;
use crate::interpreter::{Env, Environment, Interpreter};
use crate::value::{PropAttrs, Value};
use std::rc::Rc;

pub fn setup_builtins(env: &Env) {
    let mut e = env.borrow_mut();

    for name in [
        "Boolean",
        "Map",
        "Set",
        "WeakMap",
        "WeakSet",
        "WeakRef",
        "FinalizationRegistry",
        "ArrayBuffer",
        "DataView",
        "SharedArrayBuffer",
        "Atomics",
        "RegExp",
        "Function",
        "Proxy",
        "undefined",
        "isNaN",
        "isFinite",
        "parseInt",
        "parseFloat",
        "encodeURI",
        "decodeURI",
        "encodeURIComponent",
        "decodeURIComponent",
        "escape",
        "unescape",
        "eval",
        "Object",
        "Array",
        "String",
        "Number",
        "Promise",
        "Date",
        "BigInt",
        "Reflect",
        "Intl",
        "JSON",
    ] {
        e.set(name, Value::object(vec![]));
    }
    e.set("globalThis", Value::GlobalObject);

    e.set(
        "Math",
        Value::object(vec![
            ("PI".to_string(), Value::Number(std::f64::consts::PI)),
            ("E".to_string(), Value::Number(std::f64::consts::E)),
            ("LN2".to_string(), Value::Number(std::f64::consts::LN_2)),
            ("LN10".to_string(), Value::Number(std::f64::consts::LN_10)),
            ("LOG2E".to_string(), Value::Number(std::f64::consts::LOG2_E)),
            (
                "LOG10E".to_string(),
                Value::Number(std::f64::consts::LOG10_E),
            ),
            (
                "SQRT1_2".to_string(),
                Value::Number(std::f64::consts::FRAC_1_SQRT_2),
            ),
            ("SQRT2".to_string(), Value::Number(std::f64::consts::SQRT_2)),
            ("abs".to_string(), Value::Undefined),
            ("floor".to_string(), Value::Undefined),
            ("ceil".to_string(), Value::Undefined),
            ("round".to_string(), Value::Undefined),
            ("sqrt".to_string(), Value::Undefined),
            ("pow".to_string(), Value::Undefined),
            ("min".to_string(), Value::Undefined),
            ("max".to_string(), Value::Undefined),
            ("random".to_string(), Value::Undefined),
        ]),
    );

    install_functions(&mut e);

    e.set("Infinity", Value::Number(f64::INFINITY));
    e.set("NaN", Value::Number(f64::NAN));
    e.set("undefined", Value::Undefined);
    e.set(
        "eval",
        nf("eval", |interp, _, args| match args.first() {
            Some(Value::String(source)) => {
                let mut parser = crate::Parser::new_with_spans(
                    crate::Lexer::from_js_string(source).tokenize_with_spans(),
                );
                let body = parser
                    .parse_program()
                    .map_err(|e| VmErr::Msg(format!("SyntaxError: {}", e.message)))?;
                interp.run_program_body(&body)
            }
            Some(value) => Ok(value.clone()),
            None => Ok(Value::Undefined),
        }),
    );
}

/// Overwrite the placeholder members above with real native implementations.
fn install_functions(e: &mut crate::interpreter::Environment) {
    math::install(e);
    object::install(e);
    function::install(e);
    array::install(e);
    string::install(e);
    number::install(e);
    json::install(e);
    date::install(e);
    error::install(e);
    promise::install(e);
    reflect::install(e);
    collections::install(e);
    weak::install(e);
    regexp::install(e);
    bigint::install(e);
    typedarray::install(e);
    proxy::install(e);
    symbol::install(e);
    // Global functions.
    e.set("parseInt", nf("parseInt", number::parse_int));
    e.set("parseFloat", nf("parseFloat", number::parse_float));
    e.set("isNaN", nf("isNaN", global_is_nan));
    e.set("isFinite", nf("isFinite", global_is_finite));
}

/// Give a callable built-in its own prototype object and the standard
/// `Function.prototype` parent. The prototype object itself is prepared by
/// the builtin installer so each constructor can define its own methods.
pub fn set_builtin_constructor_prototype(e: &Environment, constructor: &Value, prototype: Value) {
    constructor
        .set_prop("prototype".into(), prototype)
        .expect("built-in constructor prototype");
    if let Value::Object { props } = constructor {
        props.meta.borrow_mut().set_attrs(
            "prototype",
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
        if let Some(function_prototype) = e
            .get("Function")
            .and_then(|function| function.get_prop("prototype"))
        {
            props.set_proto(Some(std::rc::Rc::new(function_prototype)));
        }
    }
}

/// Install the shared prototype used by boxed primitive values. The wrapper
/// value is also the prototype object for string, number, boolean, bigint,
/// and symbol, so the VM can preserve primitive identity while exposing the
/// ordinary `Object.getPrototypeOf` relationship.
pub(crate) fn install_primitive_prototype(
    e: &Environment,
    constructor: &Value,
    primitive: Value,
    methods: Vec<(&str, Value)>,
) {
    let object_prototype = e
        .get("Object")
        .and_then(|object| object.get_prop("prototype"))
        .map(Rc::new);
    let prototype = Value::boxed_primitive(primitive)
        .expect("primitive constructors install primitive prototype values");
    if let Value::Object { props } = &prototype {
        props.set_proto(object_prototype);
    }
    prototype
        .set_prop("constructor".into(), constructor.clone())
        .expect("primitive prototype constructor");
    for (name, method) in &methods {
        prototype
            .set_prop((*name).into(), method.clone())
            .expect("primitive prototype method");
    }
    if let Value::Object { props } = &prototype {
        let mut meta = props.meta.borrow_mut();
        meta.set_attrs(
            "constructor",
            PropAttrs {
                enumerable: false,
                ..PropAttrs::default()
            },
        );
        for (name, _) in &methods {
            meta.set_attrs(
                name,
                PropAttrs {
                    enumerable: false,
                    ..PropAttrs::default()
                },
            );
        }
    }
    set_builtin_constructor_prototype(e, constructor, prototype);
}

// ===========================================================================
// Shared helpers for the native function implementations in the sub-modules.
// ===========================================================================

pub type NativeFn = fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>;

fn nf(name: &str, callable: NativeFn) -> Value {
    Value::NativeFunction {
        name: name.into(),
        callable,
    }
}

/// A native method with ordinary, mutable function property descriptors.
fn native_method(name: &str, length: usize, callable: NativeFn, prototype: Option<Value>) -> Value {
    let properties = crate::heap::tracked(std::rc::Rc::new(crate::value::ObjectCell::new(
        vec![
            ("name".into(), Value::String(name.into())),
            ("length".into(), Value::Number(length as f64)),
        ],
        prototype.map(std::rc::Rc::new),
    )));
    for key in ["name", "length"] {
        properties.meta.borrow_mut().set_attrs(
            key,
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    Value::Function(std::rc::Rc::new(crate::value::FunctionData {
        native: Some(callable),
        identity: std::rc::Rc::new(0),
        name: Some(name.into()),
        properties,
        standard_properties_initialized: std::rc::Rc::new(std::cell::Cell::new(true)),
        params: std::rc::Rc::new(Vec::new()),
        body: std::rc::Rc::new(Vec::new()),
        closure: None,
        is_arrow: false,
        is_constructor: false,
        is_async: false,
        is_generator: false,
        uses_arguments: false,
        needs_hoisting: false,
        bound: None,
        bytecode: None,
    }))
}

/// Make a built-in namespace object callable.
///
/// `String`, `Number`, `Array` and friends are objects so they can carry their
/// statics, but they are also functions. Installing the implementation in an
/// internal slot lets `Interpreter::call_this` and `Interpreter::ctor` find it
/// while keeping the statics where property access expects them. `construct`
/// is only needed where `new X(…)` differs from `X(…)`.
pub fn make_callable(target: &Value, call: NativeFn, construct: Option<NativeFn>) {
    target
        .set_prop(
            crate::interpreter::call::CALL_SLOT.to_string(),
            nf("call", call),
        )
        .expect("built-in call slot");
    if let Some(construct) = construct {
        target
            .set_prop(
                crate::interpreter::call::CONSTRUCT_SLOT.to_string(),
                nf("construct", construct),
            )
            .expect("built-in construct slot");
    }
}

fn arr_items(this: &Value) -> Vec<Value> {
    match this {
        Value::Array(a) => a.borrow().clone(),
        _ => vec![],
    }
}

fn str_this(interp: &mut Interpreter, this: &Value) -> Result<crate::JsString, VmErr> {
    if matches!(this, Value::Null | Value::Undefined) {
        return Err(VmErr::Msg(
            "TypeError: String receiver is null or undefined".into(),
        ));
    }
    if matches!(this, Value::Symbol(_)) {
        return Err(VmErr::Msg(
            "TypeError: Cannot convert a Symbol value to a string".into(),
        ));
    }
    if matches!(this, Value::String(_))
        || matches!(this, Value::Object { props } if props.meta.borrow().boxed_primitive.is_some())
    {
        return interp.to_js_string(this);
    }
    interp.display_string(this)
}
fn join_str(interp: &Interpreter, v: &Value) -> Result<crate::JsString, VmErr> {
    match v {
        Value::Null | Value::Undefined => Ok(crate::JsString::default()),
        _ => interp.to_js_string(v),
    }
}

// --- Global functions -------------------------------------------------------

fn global_is_nan(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Bool(n.is_nan()))
}
fn global_is_finite(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Bool(n.is_finite()))
}

#[cfg(feature = "napi")]
pub(crate) use collections::{CollectionContext, clear_collection_cache};
#[cfg(feature = "napi")]
pub(crate) use symbol::SymbolContext;

#[cfg(test)]
pub use crate::test_support::interpreter as with_runtime_builtins;
