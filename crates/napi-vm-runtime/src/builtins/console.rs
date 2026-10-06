use super::*;
pub fn install_console(e: &mut Environment) {
    e.set("console", Value::object(vec![]));
    // console: route output to the host's stdout/stderr.
    if let Some(c) = e.get("console") {
        c.set_prop("log".to_string(), nf("log", console_out))
            .expect("built-in console property");
        c.set_prop("info".to_string(), nf("info", console_out))
            .expect("built-in console property");
        c.set_prop("debug".to_string(), nf("debug", console_out))
            .expect("built-in console property");
        c.set_prop("error".to_string(), nf("error", console_err))
            .expect("built-in console property");
        c.set_prop("warn".to_string(), nf("warn", console_err))
            .expect("built-in console property");
        c.set_prop("dir".to_string(), nf("dir", console_dir))
            .expect("built-in console property");
    }
}

// --- console ----------------------------------------------------------------

/// Format console arguments the way `console.log` does: each value stringified
/// and joined with a single space.
fn console_fmt(interp: &mut Interpreter, a: &[Value]) -> Result<String, VmErr> {
    let mut output = crate::format::BoundedOutput::new(crate::value::MAX_STRING_LEN);
    for (index, value) in a.iter().enumerate() {
        if index > 0 {
            output.push_char(' ')?;
        }
        let rendered = interp.display_string(value)?;
        output.push_str(&rendered)?;
    }
    Ok(output.finish())
}

fn console_out(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    println!("{}", console_fmt(interp, &a)?);
    Ok(Value::Undefined)
}

fn console_err(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    eprintln!("{}", console_fmt(interp, &a)?);
    Ok(Value::Undefined)
}

/// `console.dir`: print each value with the pretty, multi-line, indented
/// expander (`bindings::to_string_pretty`) — the sandbox-native analogue of
/// Node's `util.inspect`. Nested objects/arrays render as an indented tree
/// instead of the opaque `[object Object]` that `console.log` uses.
/// Cycle- and depth-safe by construction of that formatter.
///
/// Values are type-colored (keys cyan, strings green, numbers blue, booleans
/// yellow, null/undefined dimmed) whenever stdout is a TTY, honoring
/// `NO_COLOR`/`FORCE_COLOR`. Like Node, an options object overrides the
/// auto-detection: `console.dir(obj, { colors: true })` forces ANSI codes
/// even into a pipe, `{ colors: false }` suppresses them.
fn console_dir(_interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    // Read the boolean options out of a trailing options object, if present.
    let colors_opt = match a.get(1) {
        Some(Value::Object { props, .. }) => {
            let b = props.borrow();
            b.iter()
                .find(|(k, _)| k == "colors")
                .and_then(|(_, v)| match v {
                    Value::Bool(x) => Some(*x),
                    _ => None,
                })
        }
        _ => None,
    };
    let colors = colors_opt.unwrap_or_else(crate::format::colors_enabled);

    // Only the values are printed; a trailing options object is not a value
    // to inspect (matches Node's `console.dir(obj, options)` signature).
    let values = if matches!(a.get(1), Some(Value::Object { .. })) && a.len() == 2 {
        &a[..1]
    } else {
        &a[..]
    };

    let mut output = crate::format::BoundedOutput::new(crate::value::MAX_STRING_LEN);
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_char(' ')?;
        }
        let rendered = crate::format::try_to_string_pretty_colored(value, colors)?;
        output.push_str(&rendered)?;
    }
    println!("{}", output.finish());
    Ok(Value::Undefined)
}
