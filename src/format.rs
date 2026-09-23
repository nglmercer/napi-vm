//! String rendering of VM values: the plain `to_string` coercer, the
//! multi-line `util.inspect`-style pretty printer, ANSI color support,
//! and the [`Printer`] class with delegated element/subelement rendering.
//! Used by `console.dir`, the NAPI return-value stringification,
//! and the async bridge's error formatting.
use std::rc::Rc;

use crate::error::VmErr;
use crate::value::{MAX_STRING_LEN, Value, limit_err};

/// A single output buffer shared by all guest-controlled renderers. Every
/// append is checked before touching the allocation, so a shared object graph
/// can spend time rendering repeated references but can never grow the host
/// output past the configured cap.
pub(crate) struct BoundedOutput {
    output: String,
    max_len: usize,
}

impl BoundedOutput {
    pub(crate) fn new(max_len: usize) -> Self {
        Self {
            output: String::new(),
            max_len,
        }
    }

    pub(crate) fn push_str(&mut self, text: &str) -> Result<(), VmErr> {
        if self.output.len().saturating_add(text.len()) > self.max_len {
            return Err(limit_err("Maximum string length exceeded"));
        }
        self.output.push_str(text);
        Ok(())
    }

    pub(crate) fn push_char(&mut self, ch: char) -> Result<(), VmErr> {
        if self.output.len().saturating_add(ch.len_utf8()) > self.max_len {
            return Err(limit_err("Maximum string length exceeded"));
        }
        self.output.push(ch);
        Ok(())
    }

    pub(crate) fn finish(self) -> String {
        self.output
    }
}

/// Maximum nesting `to_string` renders before abbreviating. Together with
/// the visited set this makes stringifying any guest structure total:
/// cyclic values print `[Circular]` and very deep ones print `[Object]` /
/// `[Array]` instead of overflowing the native stack.
const MAX_PRINT_DEPTH: usize = 128;

/// Options that control how [`Printer`] renders values.
#[derive(Clone, Debug)]
pub struct PrintOptions {
    /// Spaces used per indentation level (default 2).
    pub indent: usize,
    /// Maximum nesting depth before abbreviating to `[Object]` / `[Array]`
    /// (default 128).
    pub max_depth: usize,
    /// Emit ANSI type colors when true; when false no codes are emitted.
    pub colors: bool,
}

impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            indent: 2,
            max_depth: MAX_PRINT_DEPTH,
            colors: false,
        }
    }
}

/// Stateful, configurable value printer that delegates element and
/// subelement rendering back to itself — analogous to Node's `util.inspect`.
///
/// The printer owns its visited-set so it can be reused across multiple
/// values without re-allocating the hash set, and its `PrintOptions` let
/// callers choose indentation, depth cap, and ANSI coloring in a single
/// configuration pass.
pub struct Printer {
    options: PrintOptions,
    visited: std::collections::HashSet<*const ()>,
}

impl Printer {
    /// Construct a printer from the given options.
    pub fn new(options: PrintOptions) -> Self {
        Self {
            options,
            visited: std::collections::HashSet::new(),
        }
    }

    /// Create a printer with default options and no colors.
    pub fn plain() -> Self {
        Self::new(PrintOptions::default())
    }

    /// Create a printer with colors enabled when `colors` is true.
    pub fn colored(colors: bool) -> Self {
        Self::new(PrintOptions {
            colors,
            ..PrintOptions::default()
        })
    }

    /// Return a short string representation of `val` — the same output
    /// as the legacy `to_string` function.
    pub fn print(&mut self, val: &Value) -> String {
        self.try_print(val)
            .unwrap_or_else(|error| error.to_string())
    }

    pub fn try_print(&mut self, val: &Value) -> Result<String, VmErr> {
        self.visited.clear();
        let mut output = BoundedOutput::new(MAX_STRING_LEN);
        self.render_value(val, 0, false, &mut output)?;
        Ok(output.finish())
    }

    /// Return a multi-line, indented rendering of `val` — the same output
    /// as the legacy `to_string_pretty` function.
    pub fn print_pretty(&mut self, val: &Value) -> String {
        self.try_print_pretty(val)
            .unwrap_or_else(|error| error.to_string())
    }

    pub fn try_print_pretty(&mut self, val: &Value) -> Result<String, VmErr> {
        self.visited.clear();
        let mut output = BoundedOutput::new(MAX_STRING_LEN);
        self.render_value(val, 0, true, &mut output)?;
        Ok(output.finish())
    }

    /// Return a multi-line, indented, ANSI-colored rendering of `val` —
    /// equivalent to calling `to_string_pretty_colored(val, true)`.
    pub fn print_colored(&mut self, val: &Value) -> String {
        self.try_print_colored(val)
            .unwrap_or_else(|error| error.to_string())
    }

    pub fn try_print_colored(&mut self, val: &Value) -> Result<String, VmErr> {
        let saved = self.options.colors;
        self.options.colors = true;
        let result = self.try_print_pretty(val);
        self.options.colors = saved;
        result
    }

    // ---- internal delegated rendering ------------------------------------

    fn render_value(
        &mut self,
        v: &Value,
        depth: usize,
        pretty: bool,
        output: &mut BoundedOutput,
    ) -> Result<(), VmErr> {
        let mut context = InspectContext {
            visited: &mut self.visited,
            output,
            indent: self.options.indent,
            max_depth: self.options.max_depth,
            colors: self.options.colors,
        };
        render_inspect_value(v, depth, pretty, &mut context)
    }
}

/// Fallible plain rendering with the normal VM string cap.
pub fn try_to_string(val: &Value) -> Result<String, VmErr> {
    try_to_string_with_limit(val, MAX_STRING_LEN)
}

/// Fallible plain rendering with an explicit byte limit. The output is built
/// directly in one checked buffer; no child `String` collection is created.
pub fn try_to_string_with_limit(val: &Value, max_len: usize) -> Result<String, VmErr> {
    let mut output = BoundedOutput::new(max_len);
    let mut visited = std::collections::HashSet::new();
    render_plain_value(val, &mut visited, 0, &mut output)?;
    Ok(output.finish())
}

/// Compatibility wrapper for callers that historically expected an
/// infallible formatter. VM/N-API result paths use [`try_to_string`] so a
/// limit remains a catchable error; this wrapper still never allocates beyond
/// the cap and returns the limit error text on overflow.
pub fn to_string(val: &Value) -> String {
    try_to_string(val).unwrap_or_else(|error| error.to_string())
}

fn render_plain_value(
    v: &Value,
    visited: &mut std::collections::HashSet<*const ()>,
    depth: usize,
    output: &mut BoundedOutput,
) -> Result<(), VmErr> {
    match v {
        // A live module binding renders as the value it names.
        Value::Binding(cell) => render_plain_value(&cell.borrow(), visited, depth, output),
        Value::RegExp(re) => output.push_str(&format!("/{}/{}", re.regex.source, re.regex.flags)),
        // A BigInt renders with an `n` suffix when inspected, matching the
        // literal syntax; plain string coercion drops it.
        Value::BigInt(value) => output.push_str(&value.to_decimal()),
        Value::Date(ms) => output.push_str(&crate::builtins::iso_string(ms.get())),
        // A proxy renders as its target: it is meant to stand in for it.
        Value::Proxy(proxy) => render_plain_value(&proxy.target, visited, depth, output),
        Value::ArrayBuffer(bytes) => {
            output.push_str(&format!("[object ArrayBuffer({})]", bytes.borrow().len()))
        }
        Value::TypedArray(view) => output.push_str(&format!(
            "{}({})",
            view.kind.name(),
            view.effective_length()
        )),
        Value::DataView(view) => {
            output.push_str(&format!("[object DataView({})]", view.effective_length()))
        }
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => output.push_str("[object AsyncTask]"),
        Value::Undefined => output.push_str("undefined"),
        Value::Null => output.push_str("null"),
        Value::Bool(b) => output.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => output.push_str(&number_string(*n)),
        Value::String(s) => output.push_str(s),
        Value::Object { props, .. } => {
            if depth >= MAX_PRINT_DEPTH {
                return output.push_str("[Object]");
            }
            let ptr = Rc::as_ptr(props) as *const ();
            if !visited.insert(ptr) {
                return output.push_str("[Circular]");
            }
            let result = (|| {
                output.push_char('{')?;
                let props = props.borrow();
                for (index, (key, value)) in props.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ")?;
                    }
                    output.push_str(key)?;
                    output.push_str(": ")?;
                    render_plain_value(value, visited, depth + 1, output)?;
                }
                output.push_char('}')
            })();
            visited.remove(&ptr);
            result
        }
        Value::Array(items) => {
            if depth >= MAX_PRINT_DEPTH {
                return output.push_str("[Array]");
            }
            let ptr = Rc::as_ptr(items) as *const ();
            if !visited.insert(ptr) {
                return output.push_str("[Circular]");
            }
            let result = (|| {
                output.push_char('[')?;
                let items = items.borrow();
                for (index, value) in items.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ")?;
                    }
                    render_plain_value(value, visited, depth + 1, output)?;
                }
                output.push_char(']')
            })();
            visited.remove(&ptr);
            result
        }
        Value::Function(f) => {
            output.push_str("[Function: ")?;
            output.push_str(f.name.as_deref().unwrap_or("anonymous"))?;
            output.push_char(']')
        }
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            output.push_str("[Function: ")?;
            output.push_str(name)?;
            output.push_str(" [native]]")
        }
        Value::GlobalObject => output.push_str("[object global]"),
        Value::Class(c) => {
            output.push_str("[class ")?;
            output.push_str(&c.name)?;
            output.push_char(']')
        }
        Value::Promise { .. } | Value::HostPending { .. } => output.push_str("[object Promise]"),
        Value::Generator { .. } => output.push_str("[object Generator]"),
        Value::StringIterator { .. } => output.push_str("[object String Iterator]"),
        Value::Symbol(s) => output.push_str(&s.to_display()),
        Value::Error(e) => output.push_str(&e.message),
    }
}

pub fn number_string(n: f64) -> String {
    // Rust renders these as `inf`/`-inf`/`NaN`; JavaScript spells them out.
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{:.0}", n)
    } else {
        n.to_string()
    }
}

/// Pretty, multi-line, indented rendering of a value — the sandbox-native
/// analogue of Node's `util.inspect`. Strings are single-quoted, object keys
/// that are not valid identifiers are quoted, and compound values (objects,
/// and arrays that contain objects/arrays) break across indented lines.
/// Cycle- and depth-safe by the same visited-set + `MAX_PRINT_DEPTH` scheme
/// as `to_string`.
pub fn try_to_string_pretty(val: &Value) -> Result<String, VmErr> {
    try_to_string_pretty_colored(val, false)
}

pub fn to_string_pretty(val: &Value) -> String {
    try_to_string_pretty(val).unwrap_or_else(|error| error.to_string())
}

/// Like `to_string_pretty`, but with ANSI type colors when `colors` is on:
/// keys cyan, strings green, numbers blue, booleans yellow, and
/// `null`/`undefined`/circular markers dimmed — the same broad scheme
/// Node's `util.inspect({ colors: true })` uses.
pub fn try_to_string_pretty_colored(val: &Value, colors: bool) -> Result<String, VmErr> {
    let mut visited = std::collections::HashSet::new();
    let mut output = BoundedOutput::new(MAX_STRING_LEN);
    let mut context = InspectContext {
        visited: &mut visited,
        output: &mut output,
        indent: 2,
        max_depth: MAX_PRINT_DEPTH,
        colors,
    };
    render_inspect_value(val, 0, true, &mut context)?;
    Ok(output.finish())
}

pub fn to_string_pretty_colored(val: &Value, colors: bool) -> String {
    try_to_string_pretty_colored(val, colors).unwrap_or_else(|error| error.to_string())
}

/// Whether ANSI colors should be emitted: stdout must be a TTY unless
/// `FORCE_COLOR` is set, and `NO_COLOR` always wins (https://no-color.org).
pub fn colors_enabled() -> bool {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("FORCE_COLOR").is_some() {
        return true;
    }
    std::io::stdout().is_terminal()
}

/// Applies ANSI SGR styles around rendered tokens. Every method is a no-op
/// when `enabled` is false, so the same rendering path serves both TTY and
/// piped/redirected output without branching at each call site.
///
/// `pub(crate)` so internal formatters render tokens with the exact same
/// palette as `console.dir`.
pub(crate) struct Painter {
    enabled: bool,
}

impl Painter {
    fn write_wrapped(
        &self,
        output: &mut BoundedOutput,
        code: &str,
        text: &str,
    ) -> Result<(), VmErr> {
        if self.enabled {
            output.push_str("\x1b[")?;
            output.push_str(code)?;
            output.push_str("m")?;
        }
        output.push_str(text)?;
        if self.enabled {
            output.push_str("\x1b[0m")?;
        }
        Ok(())
    }
}

struct InspectContext<'a> {
    visited: &'a mut std::collections::HashSet<*const ()>,
    output: &'a mut BoundedOutput,
    indent: usize,
    max_depth: usize,
    colors: bool,
}

fn render_inspect_value(
    v: &Value,
    depth: usize,
    pretty: bool,
    context: &mut InspectContext<'_>,
) -> Result<(), VmErr> {
    let painter = Painter {
        enabled: context.colors && pretty,
    };
    if depth >= context.max_depth {
        return painter.write_wrapped(
            context.output,
            "2;37",
            match v {
                Value::Array(_) => "[Array]",
                Value::Object { .. } => "[Object]",
                _ => "[Object]",
            },
        );
    }
    match v {
        Value::Binding(cell) => render_inspect_value(&cell.borrow(), depth, pretty, context),
        Value::RegExp(re) => painter.write_wrapped(
            context.output,
            "31",
            &format!("/{}/{}", re.regex.source, re.regex.flags),
        ),
        Value::BigInt(value) => {
            painter.write_wrapped(context.output, "33", &format!("{}n", value.to_decimal()))
        }
        Value::Proxy(proxy) => render_inspect_value(&proxy.target, depth, pretty, context),
        Value::Date(ms) => {
            painter.write_wrapped(context.output, "35", &crate::builtins::iso_string(ms.get()))
        }
        Value::ArrayBuffer(bytes) => painter.write_wrapped(
            context.output,
            "2;37",
            &format!("ArrayBuffer({})", bytes.borrow().len()),
        ),
        Value::TypedArray(view) => painter.write_wrapped(
            context.output,
            "2;37",
            &format!("{}({})", view.kind.name(), view.effective_length()),
        ),
        Value::DataView(view) => painter.write_wrapped(
            context.output,
            "2;37",
            &format!("DataView({})", view.effective_length()),
        ),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => painter.write_wrapped(context.output, "2;37", "[object AsyncTask]"),
        Value::Undefined => painter.write_wrapped(context.output, "2;37", "undefined"),
        Value::Null => painter.write_wrapped(context.output, "1;90", "null"),
        Value::Bool(b) => {
            painter.write_wrapped(context.output, "33", if *b { "true" } else { "false" })
        }
        Value::Number(n) => {
            let s = number_string(*n);
            painter.write_wrapped(context.output, "34", &s)
        }
        Value::String(s) => {
            if !pretty {
                return context.output.push_str(s);
            }
            if context.colors {
                context.output.push_str("\x1b[32m")?;
            }
            write_quoted(context.output, s)?;
            if context.colors {
                context.output.push_str("\x1b[0m")?;
            }
            Ok(())
        }
        Value::Object { props, .. } => {
            let ptr = Rc::as_ptr(props) as *const ();
            if !context.visited.insert(ptr) {
                return painter.write_wrapped(context.output, "2;37", "[Circular]");
            }
            let result = (|| {
                let borrow = props.borrow();
                if borrow.is_empty() {
                    return context.output.push_str("{}");
                }
                context.output.push_char('{')?;
                if pretty {
                    context.output.push_char('\n')?;
                }
                for (index, (key, value)) in borrow.iter().enumerate() {
                    if pretty {
                        push_indent(context.output, context.indent, depth + 1)?;
                        write_key(context.output, key, context.colors)?;
                    } else {
                        write_key(context.output, key, false)?;
                    }
                    context.output.push_str(": ")?;
                    if pretty {
                        render_inspect_value(value, depth + 1, true, context)?;
                    } else {
                        render_plain_value(value, context.visited, depth + 1, context.output)?;
                    }
                    if index + 1 < borrow.len() {
                        context.output.push_str(if pretty { ",\n" } else { ", " })?;
                    }
                }
                if pretty {
                    context.output.push_char('\n')?;
                    push_indent(context.output, context.indent, depth)?;
                }
                context.output.push_char('}')
            })();
            context.visited.remove(&ptr);
            result
        }
        Value::Array(items) => {
            let ptr = Rc::as_ptr(items) as *const ();
            if !context.visited.insert(ptr) {
                return painter.write_wrapped(context.output, "2;37", "[Circular]");
            }
            let result = (|| {
                let borrow = items.borrow();
                if borrow.is_empty() {
                    return context.output.push_str("[]");
                }
                let has_compound = borrow
                    .iter()
                    .any(|x| matches!(x, Value::Object { .. } | Value::Array(_)));
                context.output.push_char('[')?;
                if pretty && has_compound {
                    context.output.push_char('\n')?;
                } else if !pretty {
                    context.output.push_char(' ')?;
                }
                for (index, value) in borrow.iter().enumerate() {
                    if pretty && has_compound {
                        push_indent(context.output, context.indent, depth + 1)?;
                    }
                    if pretty {
                        render_inspect_value(value, depth + 1, true, context)?;
                    } else {
                        render_plain_value(value, context.visited, depth + 1, context.output)?;
                    }
                    if index + 1 < borrow.len() {
                        context.output.push_str(if pretty && has_compound {
                            ",\n"
                        } else {
                            ", "
                        })?;
                    }
                }
                if pretty && has_compound {
                    context.output.push_char('\n')?;
                    push_indent(context.output, context.indent, depth)?;
                } else if !pretty {
                    context.output.push_char(' ')?;
                }
                context.output.push_char(']')
            })();
            context.visited.remove(&ptr);
            result
        }
        Value::Function(f) => {
            if context.colors && pretty {
                context.output.push_str("\x1b[2;37m")?;
            }
            context.output.push_str("[Function: ")?;
            context
                .output
                .push_str(f.name.as_deref().unwrap_or("anonymous"))?;
            context.output.push_char(']')?;
            if context.colors && pretty {
                context.output.push_str("\x1b[0m")?;
            }
            Ok(())
        }
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            if context.colors && pretty {
                context.output.push_str("\x1b[2;37m")?;
            }
            context.output.push_str("[Function: ")?;
            context.output.push_str(name)?;
            context.output.push_str(" [native]]")?;
            if context.colors && pretty {
                context.output.push_str("\x1b[0m")?;
            }
            Ok(())
        }
        Value::GlobalObject => painter.write_wrapped(context.output, "2;37", "[object global]"),
        Value::Class(c) => {
            if context.colors && pretty {
                context.output.push_str("\x1b[2;37m")?;
            }
            context.output.push_str("[class ")?;
            context.output.push_str(&c.name)?;
            context.output.push_char(']')?;
            if context.colors && pretty {
                context.output.push_str("\x1b[0m")?;
            }
            Ok(())
        }
        Value::Promise { .. } | Value::HostPending { .. } => {
            painter.write_wrapped(context.output, "2;37", "[object Promise]")
        }
        Value::Generator { .. } => {
            painter.write_wrapped(context.output, "2;37", "[object Generator]")
        }
        Value::StringIterator { .. } => {
            painter.write_wrapped(context.output, "2;37", "[object String Iterator]")
        }
        Value::Symbol(s) => {
            if context.colors && pretty {
                context.output.push_str("\x1b[32m")?;
            }
            context.output.push_str(&s.to_display())?;
            if context.colors && pretty {
                context.output.push_str("\x1b[0m")?;
            }
            Ok(())
        }
        Value::Error(e) => painter.write_wrapped(context.output, "2;37", &e.message),
    }
}

fn push_indent(output: &mut BoundedOutput, indent: usize, depth: usize) -> Result<(), VmErr> {
    let count = indent.saturating_mul(depth);
    for _ in 0..count {
        output.push_char(' ')?;
    }
    Ok(())
}

fn write_key(output: &mut BoundedOutput, key: &str, colors: bool) -> Result<(), VmErr> {
    if colors {
        output.push_str("\x1b[36m")?;
    }
    let mut chars = key.chars();
    let bare = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        }
        _ => false,
    };
    if bare && !key.is_empty() {
        output.push_str(key)?;
    } else {
        write_quoted(output, key)?;
    }
    if colors {
        output.push_str("\x1b[0m")?;
    }
    Ok(())
}

fn write_quoted(output: &mut BoundedOutput, value: &str) -> Result<(), VmErr> {
    output.push_char('\'')?;
    for ch in value.chars() {
        match ch {
            '\'' => output.push_str("\\'")?,
            '\\' => output.push_str("\\\\")?,
            '\n' => output.push_str("\\n")?,
            '\r' => output.push_str("\\r")?,
            '\t' => output.push_str("\\t")?,
            c => output.push_char(c)?,
        }
    }
    output.push_char('\'')
}
