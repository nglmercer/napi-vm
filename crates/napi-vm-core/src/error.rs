use std::fmt;

use crate::span::Span;
use crate::value::{ErrorData, Value};

/// A single frame in the call stack trace.
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// Shared so per-call frame pushes/pop-and-snapshot never allocate for
    /// the name: function values already own an `Rc<str>` and cloning one is
    /// a refcount bump.
    pub name: std::rc::Rc<str>,
    pub span: Span,
}

impl fmt::Display for StackFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.span.is_unknown() {
            write!(f, "    at {}", self.name)
        } else {
            write!(f, "    at {} ({})", self.name, self.span)
        }
    }
}

/// Payload of `VmErr::RuntimeError`, boxed so the error enum — and therefore
/// the `Result<Value, VmErr>` returned by every eval function — stays small
/// on the success path. The payload is only ever constructed when an error
/// actually occurred, so the extra allocation is cold-path.
#[derive(Debug)]
pub struct RuntimeErrorData {
    pub realm: Option<crate::interpreter::Env>,
    pub message: String,
    pub span: Option<Span>,
    pub stack: Vec<StackFrame>,
}

impl RuntimeErrorData {
    pub(crate) fn guest_value(&self) -> Value {
        let _realm = crate::interpreter::realm::AllocationRealm::enter(self.realm.clone());
        error_value_with_stack(&self.message, &self.stack)
    }
}

#[derive(Debug)]
pub enum VmErr {
    Ret(Value),
    /// A value thrown by user code (`throw expr`). Carries the original value
    /// so `catch (e)` can inspect thrown objects (e.g. `e.message`).
    Throw(Value),
    Msg(String),
    /// A runtime error with source location context.
    RuntimeError(Box<RuntimeErrorData>),
    /// Control-flow signal for `break`, with an optional target label. Caught
    /// by the enclosing loop/switch; not an error and not catchable by `try`.
    Break(Option<String>),
    /// Control-flow signal for `continue`, with an optional target label.
    Continue(Option<String>),
    /// Internal teardown signal: a suspended generator or async body whose
    /// last handle was dropped is resumed once with `GenResume::Abandon`, and
    /// every suspend point converts that into this. It propagates outward
    /// like an error but must never run guest code on the way out — no
    /// `catch`, no `finally`, no iterator `close` — so an abandoned body is
    /// torn down purely by dropping its frames, and the coroutine completes
    /// with a normal return instead of a cross-stack forced unwind (which the
    /// Windows unwinder cannot walk: `STATUS_ACCESS_VIOLATION`).
    ///
    /// This is never guest-visible: the `Drop` that initiates the abandon
    /// consumes the resulting `GenOutcome::Abandon`. Any other site that
    /// matches on `VmErr` must propagate it untouched.
    Abandon,
}

// Guard the hot-path size: every eval function returns this `Result`. If it
// grows, the whole interpreter slows down — box the offending payload.
const _: () = assert!(std::mem::size_of::<Result<Value, VmErr>>() <= 48);

impl VmErr {
    /// Whether this is the internal abandon-teardown signal, which skips
    /// every guest-code handler (`catch`, `finally`, iterator `close`) on
    /// its way out of an abandoned body.
    pub fn is_abandon(&self) -> bool {
        matches!(self, VmErr::Abandon)
    }

    /// Attach source location and call stack to a `VmErr::Msg`.
    pub fn with_context(self, span: Option<Span>, stack: &[StackFrame]) -> Self {
        match self {
            VmErr::Msg(message) => VmErr::RuntimeError(Box::new(RuntimeErrorData {
                realm: crate::interpreter::realm::allocation_global(),
                message,
                span,
                stack: stack.to_vec(),
            })),
            other => other,
        }
    }
}

impl fmt::Display for VmErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VmErr::Msg(s) => write!(f, "{}", s),
            VmErr::RuntimeError(inner) => {
                let RuntimeErrorData {
                    message,
                    span,
                    stack,
                    ..
                } = inner.as_ref();
                write!(f, "{}", message)?;
                if let Some(span) = span
                    && !span.is_unknown()
                {
                    write!(f, "\n  at {}", span)?;
                }
                for frame in stack.iter().rev() {
                    write!(f, "\n{}", frame)?;
                }
                Ok(())
            }
            VmErr::Throw(v) => write!(f, "{}", throw_display(v)),
            VmErr::Ret(_) => write!(f, "return"),
            VmErr::Break(Some(l)) => write!(f, "break outside loop (label {})", l),
            VmErr::Break(None) => write!(f, "break outside loop"),
            VmErr::Continue(Some(l)) => write!(f, "continue outside loop (label {})", l),
            VmErr::Continue(None) => write!(f, "continue outside loop"),
            // Internal only: the initiating `Drop` consumes this before it
            // can reach any rendering. Present so the match stays exhaustive.
            VmErr::Abandon => write!(f, "abandoned generator cleanup"),
        }
    }
}

/// Render a thrown value as an error message without needing an interpreter
/// (used when an uncaught throw crosses the NAPI boundary).
fn throw_display(v: &Value) -> String {
    use crate::value::MAX_STRING_LEN;

    fn append(out: &mut String, text: &str) -> bool {
        if out.len().saturating_add(text.len()) > MAX_STRING_LEN {
            return false;
        }
        out.push_str(text);
        true
    }

    let mut out = String::new();
    let ok = match v {
        Value::String(s) => append(&mut out, s),
        Value::Error(inner) => {
            let mut ok = true;
            if inner.name != "Error" {
                ok &= append(&mut out, &inner.name);
                ok &= append(&mut out, ": ");
            }
            ok &= append(&mut out, &inner.message);
            ok
        }
        Value::Object { props, .. } => {
            let borrow = props.borrow();
            let name = borrow.iter().find_map(|(key, value)| {
                (key == "name")
                    .then_some(value)
                    .and_then(|value| match value {
                        Value::String(value) => Some(value.as_str()),
                        _ => None,
                    })
            });
            let message = borrow.iter().find_map(|(key, value)| {
                (key == "message")
                    .then_some(value)
                    .and_then(|value| match value {
                        Value::String(value) => Some(value.as_str()),
                        _ => None,
                    })
            });
            match (name, message) {
                (Some(name), Some(message)) if name != "Error" => {
                    append(&mut out, name) && append(&mut out, ": ") && append(&mut out, message)
                }
                (_, Some(message)) => append(&mut out, message),
                _ => append(&mut out, "Uncaught error"),
            }
        }
        Value::Number(n) => append(&mut out, &n.to_string()),
        Value::Bool(b) => append(&mut out, if *b { "true" } else { "false" }),
        Value::Null => append(&mut out, "null"),
        Value::Undefined => append(&mut out, "undefined"),
        _ => append(&mut out, "error"),
    };
    if ok {
        out
    } else {
        "RangeError: Maximum string length exceeded".to_string()
    }
}

/// Build the guest-visible error value for an internal error message.
/// Messages may carry a `"Name: message"` prefix naming one of the standard
/// error types (as produced by `limit_err` and the interpreter guards);
/// anything else becomes a plain `Error`. This is what lets guest code do
/// `try { ... } catch (e) { e.message }` on internally raised errors.
pub fn error_value_from_msg(message: &str) -> Value {
    error_value_with_stack(message, &[])
}

/// As [`error_value_from_msg`], but recording the call stack the error was
/// raised on, so `e.stack` can name the frames.
pub fn error_value_with_stack(message: &str, frames: &[StackFrame]) -> Value {
    const NAMES: &[&str] = &[
        "TypeError",
        "RangeError",
        "SyntaxError",
        "ReferenceError",
        "EvalError",
        "URIError",
        "Error",
    ];
    let (name, text) = NAMES
        .iter()
        .find_map(|n| {
            message
                .strip_prefix(n)
                .and_then(|rest| rest.strip_prefix(": "))
                .map(|rest| (*n, rest))
        })
        .unwrap_or(("Error", message));
    let mut error = ErrorData::new(name, text);
    error.stack = render_stack(name, text, frames).into();
    Value::Error(error)
}

/// Render a stack the way engines print one: the error's own line, then a
/// frame per line, innermost first.
pub fn render_stack(name: &str, message: &str, frames: &[StackFrame]) -> String {
    let mut out = if message.is_empty() {
        name.to_string()
    } else {
        format!("{}: {}", name, message)
    };
    for frame in frames.iter().rev() {
        // A frame's span is unknown for a native call; omit the position
        // rather than print a misleading `0:0`.
        if frame.span.line == 0 {
            out.push_str(&format!("\n    at {}", frame.name));
        } else {
            out.push_str(&format!(
                "\n    at {} ({}:{})",
                frame.name, frame.span.line, frame.span.col
            ));
        }
    }
    out
}

pub fn vm_ret(v: Value) -> Result<Value, VmErr> {
    Err(VmErr::Ret(v))
}

pub fn vm_throw(v: Value) -> Result<Value, VmErr> {
    Err(VmErr::Throw(v))
}

pub fn vm_err<T: Into<String>>(msg: T) -> Result<Value, VmErr> {
    Err(VmErr::Msg(msg.into()))
}
