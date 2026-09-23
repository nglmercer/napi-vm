//! `JSON.stringify` / `JSON.parse`

use super::nf;
use crate::error::{VmErr, vm_err};
use crate::interpreter::{Environment, Interpreter};
use crate::value::{BoxedPrimitive, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(j) = e.get("JSON") {
        j.set_prop("stringify".to_string(), nf("stringify", json_stringify))
            .expect("built-in JSON property");
        j.set_prop("parse".to_string(), nf("parse", json_parse))
            .expect("built-in JSON property");
    }
}

/// Maximum nesting `JSON.stringify` / `JSON.parse` will walk. Real engines
/// throw a `RangeError` here; without a limit a million-deep structure
/// overflows the native stack.
const MAX_JSON_DEPTH: usize = 512;

fn json_stringify(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if matches!(v, Value::Undefined) {
        return Ok(Value::Undefined);
    }
    let mut out = String::new();
    // Path-based visited set (Rc pointer identity) so cyclic structures
    // throw a catchable TypeError — matching `JSON.stringify` in V8 —
    // instead of recursing until the native stack overflows.
    let mut visited: std::collections::HashSet<*const ()> = std::collections::HashSet::new();
    json_serialize(interp, &v, &mut out, &mut visited, 0)?;

    // The third argument indents the output: a number of spaces, or a literal
    // string. Re-indenting the compact form keeps one serializer.
    let indent = match a.get(2) {
        Some(Value::Number(n)) if *n >= 1.0 => " ".repeat((*n as usize).min(10)),
        Some(Value::String(s)) => s.chars().take(10).collect(),
        Some(other) if !matches!(other, Value::Undefined | Value::Null) => {
            let rendered = interp.vs(other)?;
            rendered.chars().take(10).collect()
        }
        _ => String::new(),
    };
    if indent.is_empty() {
        return Ok(Value::String(out));
    }
    Value::checked_string(reindent(&out, &indent)?)
}

/// Expand compact JSON onto indented lines.
///
/// Operating on the finished text rather than threading a width through the
/// serializer keeps one code path for both forms; the input is JSON this
/// module just produced, so the scan only has to respect string literals.
fn reindent(compact: &str, indent: &str) -> Result<String, VmErr> {
    let mut out = String::with_capacity(compact.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for c in compact.chars() {
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                depth += 1;
                out.push(c);
                out.push('\n');
                out.push_str(&indent.repeat(depth));
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                // An empty object or array stays on one line.
                if out.ends_with(&format!("\n{}", indent.repeat(depth + 1))) {
                    out.truncate(out.len() - 1 - indent.repeat(depth + 1).len());
                } else {
                    out.push('\n');
                    out.push_str(&indent.repeat(depth));
                }
                out.push(c);
            }
            ',' => {
                out.push(c);
                out.push('\n');
                out.push_str(&indent.repeat(depth));
            }
            ':' => {
                out.push(c);
                out.push(' ');
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

fn append_json_str(out: &mut String, value: &str) -> Result<(), VmErr> {
    if out.len().saturating_add(value.len()) > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    out.push_str(value);
    Ok(())
}

fn append_json_char(out: &mut String, value: char) -> Result<(), VmErr> {
    if out.len().saturating_add(value.len_utf8()) > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    out.push(value);
    Ok(())
}

fn json_serialize(
    interp: &mut Interpreter,
    v: &Value,
    out: &mut String,
    visited: &mut std::collections::HashSet<*const ()>,
    depth: usize,
) -> Result<(), VmErr> {
    if depth > MAX_JSON_DEPTH {
        return Err(VmErr::Msg(
            "RangeError: Maximum JSON depth exceeded".to_string(),
        ));
    }
    match v {
        Value::Null | Value::Undefined => append_json_str(out, "null")?,
        Value::Bool(b) => append_json_str(out, if *b { "true" } else { "false" })?,
        Value::Number(n) => {
            let text = if n.is_nan() || n.is_infinite() {
                "null".to_string()
            } else if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{n:.0}")
            } else {
                n.to_string()
            };
            append_json_str(out, &text)?;
        }
        Value::String(s) => {
            append_json_char(out, '"')?;
            escape_json(s, out)?;
            append_json_char(out, '"')?;
        }
        Value::TypedArray(view) if view.is_buffer => {
            let to_json = interp.member(v, "toJSON")?;
            if crate::interpreter::call::is_callable_value(&to_json) {
                let converted = interp.call_this(&to_json, v.clone(), Vec::new())?;
                return json_serialize(interp, &converted, out, visited, depth + 1);
            }
            json_serialize_typed_array(interp, view, out, visited, depth)?;
        }
        Value::TypedArray(view) => {
            json_serialize_typed_array(interp, view, out, visited, depth)?;
        }
        Value::ArrayBuffer(_) | Value::SharedArrayBuffer(_) | Value::DataView(_) => {
            append_json_str(out, "{}")?;
        }
        Value::Array(items) => {
            let ptr = std::rc::Rc::as_ptr(items) as *const ();
            if !visited.insert(ptr) {
                return Err(VmErr::Msg(
                    "TypeError: Converting circular structure to JSON".to_string(),
                ));
            }
            append_json_char(out, '[')?;
            let items = items.borrow();
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    append_json_char(out, ',')?;
                }
                json_serialize(interp, it, out, visited, depth + 1)?;
            }
            append_json_char(out, ']')?;
            visited.remove(&ptr);
        }
        // A `Date` serializes as its ISO string, which is what its `toJSON`
        // returns.
        Value::Date(ms) => {
            append_json_char(out, '"')?;
            escape_json(&crate::builtins::iso_string(ms.get()), out)?;
            append_json_char(out, '"')?;
        }
        // A proxy serializes as its target. Routing this through the `get`
        // trap would need the interpreter, which the serializer does not have.
        Value::Proxy(proxy) => {
            let target = proxy.target.clone();
            return json_serialize(interp, &target, out, visited, depth);
        }
        Value::Object { props, .. } => {
            let to_json = interp.member(v, "toJSON")?;
            if crate::interpreter::call::is_callable_value(&to_json) {
                let converted = interp.call_this(&to_json, v.clone(), Vec::new())?;
                return json_serialize(interp, &converted, out, visited, depth + 1);
            }
            match props.meta.borrow().boxed_primitive.clone() {
                Some(BoxedPrimitive::Bool(value)) => {
                    return json_serialize(interp, &Value::Bool(value), out, visited, depth + 1);
                }
                Some(BoxedPrimitive::Number(value)) => {
                    return json_serialize(interp, &Value::Number(value), out, visited, depth + 1);
                }
                Some(BoxedPrimitive::String(value)) => {
                    return json_serialize(interp, &Value::String(value), out, visited, depth + 1);
                }
                Some(BoxedPrimitive::BigInt(_)) => {
                    return Err(VmErr::Msg(
                        "TypeError: Do not know how to serialize a BigInt".into(),
                    ));
                }
                Some(BoxedPrimitive::Symbol(_)) | None => {}
            }
            json_serialize_object(interp, v, props, out, visited, depth)?;
        }
        _ => append_json_str(out, "null")?,
    }
    Ok(())
}

fn json_serialize_object(
    interp: &mut Interpreter,
    value: &Value,
    props: &std::rc::Rc<crate::value::ObjectCell>,
    out: &mut String,
    visited: &mut std::collections::HashSet<*const ()>,
    depth: usize,
) -> Result<(), VmErr> {
    let ptr = std::rc::Rc::as_ptr(props) as *const ();
    if !visited.insert(ptr) {
        return Err(VmErr::Msg(
            "TypeError: Converting circular structure to JSON".to_string(),
        ));
    }
    append_json_char(out, '{')?;
    let meta = props.meta.borrow();
    // `JSON.stringify` walks own enumerable string keys only, skipping the
    // VM's internal slots. Snapshot before getters execute guest code.
    let entries: Vec<(String, Value)> = props
        .borrow()
        .iter()
        .filter(|(key, _)| {
            !crate::interpreter::is_internal_key(key) && meta.attrs_of(key).enumerable
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    drop(meta);
    let mut first = true;
    for (key, slot) in entries {
        let is_getter = matches!(&slot, Value::Function(function)
        if function.name.as_ref().is_some_and(|name| {
            name.strip_prefix("get ").is_some_and(|rest| rest == key)
        }));
        let property = if is_getter {
            interp.member(value, &key)?
        } else {
            slot.deref_binding()
        };
        if matches!(property, Value::Undefined) {
            continue;
        }
        if !first {
            append_json_char(out, ',')?;
        }
        first = false;
        append_json_char(out, '"')?;
        escape_json(&key, out)?;
        append_json_str(out, "\":")?;
        json_serialize(interp, &property, out, visited, depth + 1)?;
    }
    append_json_char(out, '}')?;
    visited.remove(&ptr);
    Ok(())
}

fn json_serialize_typed_array(
    interp: &mut Interpreter,
    view: &std::rc::Rc<crate::value::TypedArrayData>,
    out: &mut String,
    visited: &mut std::collections::HashSet<*const ()>,
    depth: usize,
) -> Result<(), VmErr> {
    append_json_char(out, '{')?;
    for index in 0..view.effective_length() {
        if index > 0 {
            append_json_char(out, ',')?;
        }
        append_json_char(out, '"')?;
        append_json_str(out, &index.to_string())?;
        append_json_str(out, "\":")?;
        let value = crate::builtins::read_element(view, index).unwrap_or(Value::Undefined);
        json_serialize(interp, &value, out, visited, depth + 1)?;
    }
    append_json_char(out, '}')
}

fn escape_json(s: &str, out: &mut String) -> Result<(), VmErr> {
    for c in s.chars() {
        match c {
            '"' => append_json_str(out, "\\\"")?,
            '\\' => append_json_str(out, "\\\\")?,
            '\n' => append_json_str(out, "\\n")?,
            '\t' => append_json_str(out, "\\t")?,
            '\r' => append_json_str(out, "\\r")?,
            '\u{08}' => append_json_str(out, "\\b")?,
            '\u{0C}' => append_json_str(out, "\\f")?,
            c if (c as u32) < 0x20 => {
                append_json_str(out, &format!("\\u{:04x}", c as u32))?;
            }
            c => append_json_char(out, c)?,
        }
    }
    Ok(())
}

fn json_parse(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = match a.first() {
        Some(Value::String(s)) => s,
        _ => return vm_err("JSON.parse requires a string argument"),
    };
    if s.len() > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    JsonParser::new(s).parse()
}

/// A small recursive-descent JSON parser producing `Value`s directly, with no
/// token or AST allocation. Accepts strict JSON only (quoted keys, no trailing
/// commas), matching the semantics the previous lexer/parser-reuse approach
/// provided for well-formed JSON input.
struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// Current container nesting; bounded by `MAX_JSON_DEPTH` so a deeply
    /// nested document errors out instead of overflowing the native stack.
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            bytes: s.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }

    fn parse(mut self) -> Result<Value, VmErr> {
        let v = self.value()?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return vm_err("Invalid JSON");
        }
        Ok(v)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), VmErr> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(VmErr::Msg("Invalid JSON".to_string()))
        }
    }

    fn push_str(&mut self, out: &mut String, value: &str) -> Result<(), VmErr> {
        if out.len().saturating_add(value.len()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        out.push_str(value);
        Ok(())
    }

    fn push_char(&mut self, out: &mut String, value: char) -> Result<(), VmErr> {
        if out.len().saturating_add(value.len_utf8()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        out.push(value);
        Ok(())
    }

    fn value(&mut self) -> Result<Value, VmErr> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(VmErr::Msg(
                "RangeError: Maximum JSON depth exceeded".to_string(),
            ));
        }
        let r = self.value_inner();
        self.depth -= 1;
        r
    }

    fn value_inner(&mut self) -> Result<Value, VmErr> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => vm_err("Invalid JSON"),
        }
    }

    fn literal(&mut self, lit: &[u8], v: Value) -> Result<Value, VmErr> {
        if self.bytes.get(self.pos..self.pos + lit.len()) == Some(lit) {
            self.pos += lit.len();
            Ok(v)
        } else {
            vm_err("Invalid JSON")
        }
    }

    fn number(&mut self) -> Result<Value, VmErr> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        // The scanned range is ASCII-only (digits and punctuation), hence a
        // valid UTF-8 slice of the original input.
        let s = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| VmErr::Msg("Invalid JSON".to_string()))?;
        s.parse::<f64>()
            .map(Value::Number)
            .map_err(|_| VmErr::Msg("Invalid JSON".to_string()))
    }

    fn string(&mut self) -> Result<String, VmErr> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(VmErr::Msg("Invalid JSON".to_string())),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => self.push_char(&mut out, '"')?,
                        Some(b'\\') => self.push_char(&mut out, '\\')?,
                        Some(b'/') => self.push_char(&mut out, '/')?,
                        Some(b'b') => self.push_char(&mut out, '\u{08}')?,
                        Some(b'f') => self.push_char(&mut out, '\u{0C}')?,
                        Some(b'n') => self.push_char(&mut out, '\n')?,
                        Some(b'r') => self.push_char(&mut out, '\r')?,
                        Some(b't') => self.push_char(&mut out, '\t')?,
                        Some(b'u') => {
                            self.pos += 1;
                            let hi = self.hex4()?;
                            // Combine a UTF-16 surrogate pair into one scalar.
                            if (0xD800..0xDC00).contains(&hi)
                                && self.peek() == Some(b'\\')
                                && self.bytes.get(self.pos + 1) == Some(&b'u')
                            {
                                self.pos += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                                    self.push_char(
                                        &mut out,
                                        char::from_u32(cp).unwrap_or('\u{FFFD}'),
                                    )?;
                                } else {
                                    self.push_char(&mut out, '\u{FFFD}')?;
                                    self.push_char(
                                        &mut out,
                                        char::from_u32(lo).unwrap_or('\u{FFFD}'),
                                    )?;
                                }
                            } else {
                                self.push_char(&mut out, char::from_u32(hi).unwrap_or('\u{FFFD}'))?;
                            }
                            continue;
                        }
                        _ => return Err(VmErr::Msg("Invalid JSON".to_string())),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    // Fast path: copy a run of bytes with no quote/backslash.
                    // UTF-8 continuation bytes are >= 0x80 and can never be
                    // 0x22 or 0x5C, so the run ends on a char boundary.
                    let start = self.pos;
                    while matches!(self.peek(), Some(c) if c != b'"' && c != b'\\') {
                        self.pos += 1;
                    }
                    let run = std::str::from_utf8(&self.bytes[start..self.pos])
                        .map_err(|_| VmErr::Msg("Invalid JSON".to_string()))?;
                    self.push_str(&mut out, run)?;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, VmErr> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self
                .peek()
                .ok_or_else(|| VmErr::Msg("Invalid JSON".to_string()))?;
            self.pos += 1;
            v = v * 16
                + match c {
                    b'0'..=b'9' => (c - b'0') as u32,
                    b'a'..=b'f' => (c - b'a' + 10) as u32,
                    b'A'..=b'F' => (c - b'A' + 10) as u32,
                    _ => return Err(VmErr::Msg("Invalid JSON".to_string())),
                };
        }
        Ok(v)
    }

    fn array(&mut self) -> Result<Value, VmErr> {
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Value::checked_array(items);
        }
        loop {
            if items.len() >= crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Value::checked_array(items);
                }
                _ => return vm_err("Invalid JSON"),
            }
        }
    }

    fn object(&mut self) -> Result<Value, VmErr> {
        self.pos += 1; // {
        let mut props = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Value::checked_object(props);
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return vm_err("Invalid JSON");
            }
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            if props.len() >= crate::value::MAX_OBJECT_PROPS {
                return Err(crate::value::limit_err(
                    "Maximum object property count exceeded",
                ));
            }
            let v = self.value()?;
            props.push((key, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Value::checked_object(props);
                }
                _ => return vm_err("Invalid JSON"),
            }
        }
    }
}
