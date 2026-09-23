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
use crate::value::{FunctionData, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("Function") {
        super::make_callable(&namespace, new_function, None);
    }
}

/// Methods shared by guest and native callable values.
pub(crate) fn function_method(name: &str) -> Option<Value> {
    Some(match name {
        "call" => super::nf("call", function_call),
        _ => return None,
    })
}

fn function_call(
    interp: &mut Interpreter,
    target: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let receiver = args.first().cloned().unwrap_or(Value::Undefined);
    interp.call_this(&target, receiver, args.into_iter().skip(1).collect())
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
        params: Rc::new(params.iter().map(|p| Rc::from(p.as_str())).collect()),
        body: Rc::new(body),
        // The global scope, not the caller's: a function built from a string
        // must not capture bindings its source never named.
        closure: Some(interp.persistent_global.clone()),
        is_arrow: false,
        is_async: false,
        is_generator: false,
        uses_arguments,
    })))
}
