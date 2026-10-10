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
/// overflows the native stack. Shared with the host conversion layer so
/// both directions enforce the same bound.
pub(crate) const MAX_JSON_DEPTH: usize = 512;

fn json_stringify(
    interp: &mut Interpreter,
    _: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = arguments.first().cloned().unwrap_or(Value::Undefined);
    let replacer = arguments.get(1).cloned().unwrap_or(Value::Undefined);
    let mut state = Serialization::new(interp, &replacer)?;
    let mut space = arguments.get(2).cloned().unwrap_or(Value::Undefined);
    space = match boxed(&space) {
        Some(BoxedPrimitive::Number(_)) => Value::Number(interp.ecmascript_to_number(&space)?),
        Some(BoxedPrimitive::String(_)) => Value::String(json_string(interp, &space)?),
        _ => space,
    };
    let indent: crate::JsString = match &space {
        Value::Number(number) if *number >= 1.0 => " ".repeat((*number as usize).min(10)).into(),
        Value::String(text) => text.slice(0, text.len().min(10)),
        _ => crate::JsString::default(),
    };
    let holder = Value::checked_object(vec![(String::new(), value)])?;
    let mut out = String::new();
    if !state.serialize(interp, &holder, &crate::JsString::default(), &mut out)? {
        return Ok(Value::Undefined);
    }
    if indent.is_empty() {
        Value::checked_string(out)
    } else {
        Value::checked_string(reindent(&out, &indent)?)
    }
}

/// Expand compact JSON onto indented lines.
///
/// Operating on the finished text rather than threading a width through the
/// serializer keeps one code path for both forms; the input is JSON this
/// module just produced, so the scan only has to respect string literals.
fn reindent(compact: &str, indent: impl Into<crate::JsString>) -> Result<crate::JsString, VmErr> {
    let indent = indent.into();
    let mut out = crate::JsString::default();
    let mut depth = 0usize;
    // One cached pad string per nesting depth, built on demand. The loop
    // below touches a pad on every structural character; rebuilding
    // `indent.repeat(depth)` there costs an allocation per character.
    let mut pads: Vec<crate::JsString> = vec![crate::JsString::default()];
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
        // Every arm below indexes at most `depth + 1` (`{` increments first).
        while pads.len() <= depth + 1 {
            pads.push(pads[pads.len() - 1].concat(&indent));
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
                out.push_str(&pads[depth]);
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                // An empty object or array stays on one line: the output ends
                // with a newline plus the deeper pad. This byte check is
                // exactly `out.ends_with("\n" + pad)` without the `format!`.
                let pad = &pads[depth + 1];
                let suffix_len = 1 + pad.len();
                let is_empty = out.len() >= suffix_len
                    && out.units()[out.len() - suffix_len] == 10
                    && out.units().ends_with(pad.units());
                if is_empty {
                    out.truncate(out.len() - suffix_len);
                } else {
                    out.push('\n');
                    out.push_str(&pads[depth]);
                }
                out.push(c);
            }
            ',' => {
                out.push(c);
                out.push('\n');
                out.push_str(&pads[depth]);
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

struct Serialization {
    replacer: Option<Value>,
    property_list: Option<std::rc::Rc<Vec<crate::JsString>>>,
    stack: Vec<Value>,
}

enum SerializedProperty {
    Absent,
    Written,
    Object(Value),
}

enum SerializationFrame {
    Array {
        value: Value,
        length: usize,
        index: usize,
    },
    Object {
        value: Value,
        keys: std::rc::Rc<Vec<crate::JsString>>,
        index: usize,
        first: bool,
    },
}

fn json_length(interp: &mut Interpreter, object: &Value) -> Result<usize, VmErr> {
    let length = interp.get_prop_value_str(object, "length")?;
    let number = interp.ecmascript_to_number(&length)?;
    if number.is_nan() || number <= 0.0 {
        return Ok(0);
    }
    if number > crate::value::MAX_ARRAY_LEN as f64 {
        return Err(crate::value::limit_err(
            "Maximum JSON array length exceeded",
        ));
    }
    Ok(number.trunc() as usize)
}

fn boxed(value: &Value) -> Option<BoxedPrimitive> {
    value
        .property_cell()
        .and_then(|cell| cell.meta.borrow().boxed_primitive.clone())
}

fn json_string(interp: &mut Interpreter, value: &Value) -> Result<crate::JsString, VmErr> {
    interp.ecmascript_to_string(value)
}

impl Serialization {
    fn new(interp: &mut Interpreter, replacer: &Value) -> Result<Self, VmErr> {
        let mut state = Self {
            replacer: None,
            property_list: None,
            stack: Vec::new(),
        };
        if crate::interpreter::call::is_callable_value(replacer) {
            state.replacer = Some(replacer.clone());
        } else if super::array::is_array(replacer)? {
            let length = json_length(interp, replacer)?;
            let mut keys = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for index in 0..length {
                let item = interp.get_prop_value_str(replacer, &index.to_string())?;
                if matches!(item, Value::String(_) | Value::Number(_))
                    || matches!(
                        boxed(&item),
                        Some(BoxedPrimitive::String(_) | BoxedPrimitive::Number(_))
                    )
                {
                    let key = json_string(interp, &item)?;
                    if seen.insert(key.units().to_vec()) {
                        keys.push(key);
                    }
                }
            }
            state.property_list = Some(std::rc::Rc::new(keys));
        }
        Ok(state)
    }

    fn property(
        &mut self,
        interp: &mut Interpreter,
        holder: &Value,
        key: &crate::JsString,
        out: &mut String,
        depth: usize,
    ) -> Result<SerializedProperty, VmErr> {
        if depth > MAX_JSON_DEPTH {
            return Err(VmErr::Msg("RangeError: Maximum JSON depth exceeded".into()));
        }
        let mut value = interp.get_prop_value(holder, &Value::String(key.clone()))?;
        if crate::interpreter::call::is_js_object(&value) || matches!(value, Value::BigInt(_)) {
            let to_json = interp.get_prop_value_str(&value, "toJSON")?;
            if crate::interpreter::call::is_callable_value(&to_json) {
                value =
                    interp.call_this(&to_json, value.clone(), vec![Value::String(key.clone())])?;
            }
        }
        if let Some(replacer) = &self.replacer {
            value = interp.call_this(
                replacer,
                holder.clone(),
                vec![Value::String(key.clone()), value],
            )?;
        }
        value = match boxed(&value) {
            Some(BoxedPrimitive::Number(_)) => Value::Number(interp.ecmascript_to_number(&value)?),
            Some(BoxedPrimitive::String(_)) => Value::String(json_string(interp, &value)?),
            Some(BoxedPrimitive::Bool(value)) => Value::Bool(value),
            Some(BoxedPrimitive::BigInt(value)) => Value::BigInt(value),
            _ => value,
        };
        match &value {
            Value::Null => append_json_str(out, "null")?,
            Value::Bool(value) => append_json_str(out, if *value { "true" } else { "false" })?,
            Value::Number(value) => {
                if value.is_finite() {
                    append_json_str(out, &crate::format::ecmascript_number_string(*value))?;
                } else {
                    append_json_str(out, "null")?;
                }
            }
            Value::String(value) => {
                append_json_char(out, '"')?;
                escape_json(value, out)?;
                append_json_char(out, '"')?;
            }
            Value::BigInt(_) => {
                return Err(VmErr::Msg(
                    "TypeError: Do not know how to serialize a BigInt".into(),
                ));
            }
            _ if !crate::interpreter::call::is_js_object(&value)
                || crate::interpreter::call::is_callable_value(&value) =>
            {
                return Ok(SerializedProperty::Absent);
            }
            _ => return Ok(SerializedProperty::Object(value)),
        }
        Ok(SerializedProperty::Written)
    }

    fn open(
        &mut self,
        interp: &mut Interpreter,
        value: Value,
        out: &mut String,
    ) -> Result<SerializationFrame, VmErr> {
        if self
            .stack
            .iter()
            .any(|item| crate::interpreter::strict_equals(item, &value))
        {
            return Err(VmErr::Msg(
                "TypeError: Converting circular structure to JSON".into(),
            ));
        }
        self.stack.push(value.clone());
        if super::array::is_array(&value)? {
            let length = json_length(interp, &value)?;
            append_json_char(out, '[')?;
            Ok(SerializationFrame::Array {
                value,
                length,
                index: 0,
            })
        } else {
            let keys = if let Some(keys) = &self.property_list {
                keys.clone()
            } else {
                let mut keys = Vec::new();
                for key in interp.own_property_keys(&value)? {
                    if let Value::String(name) = &key {
                        let descriptor =
                            super::object::descriptor_for_key_in(interp, &value, &key)?;
                        if descriptor
                            .get_prop("enumerable")
                            .is_some_and(|value| value.is_truthy())
                        {
                            keys.push(name.clone());
                        }
                    }
                }
                std::rc::Rc::new(keys)
            };
            append_json_char(out, '{')?;
            Ok(SerializationFrame::Object {
                value,
                keys,
                index: 0,
                first: true,
            })
        }
    }

    /// Keep traversal state on the heap: the JSON depth limit must remain a
    /// catchable guest error even on small owner-thread stacks.
    fn serialize(
        &mut self,
        interp: &mut Interpreter,
        holder: &Value,
        key: &crate::JsString,
        out: &mut String,
    ) -> Result<bool, VmErr> {
        let mut frames = match self.property(interp, holder, key, out, 0)? {
            SerializedProperty::Absent => return Ok(false),
            SerializedProperty::Written => return Ok(true),
            SerializedProperty::Object(value) => vec![self.open(interp, value, out)?],
        };
        while let Some(frame) = frames.last_mut() {
            let (holder, key, array, checkpoint) = match frame {
                SerializationFrame::Array {
                    value,
                    length,
                    index,
                } => {
                    if *index == *length {
                        append_json_char(out, ']')?;
                        frames.pop();
                        self.stack.pop();
                        continue;
                    }
                    if *index != 0 {
                        append_json_char(out, ',')?;
                    }
                    let key = index.to_string().into();
                    *index += 1;
                    (value.clone(), key, true, 0)
                }
                SerializationFrame::Object {
                    value,
                    keys,
                    index,
                    first,
                } => {
                    if *index == keys.len() {
                        append_json_char(out, '}')?;
                        frames.pop();
                        self.stack.pop();
                        continue;
                    }
                    let key = keys[*index].clone();
                    *index += 1;
                    let checkpoint = out.len();
                    if !*first {
                        append_json_char(out, ',')?;
                    }
                    append_json_char(out, '"')?;
                    escape_json(&key, out)?;
                    append_json_str(out, "\":")?;
                    (value.clone(), key, false, checkpoint)
                }
            };
            let property = self.property(interp, &holder, &key, out, frames.len())?;
            if matches!(property, SerializedProperty::Absent) {
                if array {
                    append_json_str(out, "null")?;
                } else {
                    out.truncate(checkpoint);
                }
                continue;
            }
            if let Some(SerializationFrame::Object { first, .. }) = frames.last_mut() {
                *first = false;
            }
            if let SerializedProperty::Object(value) = property {
                frames.push(self.open(interp, value, out)?);
            }
        }
        Ok(true)
    }
}

fn escape_json(s: impl Into<crate::JsString>, out: &mut String) -> Result<(), VmErr> {
    let s = s.into();
    for decoded in char::decode_utf16(s.units().iter().copied()) {
        let c = match decoded {
            Ok(c) => c,
            Err(e) => {
                append_json_str(out, &format!("\\u{:04x}", e.unpaired_surrogate()))?;
                continue;
            }
        };
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
    let mut encoded = String::new();
    for decoded in char::decode_utf16(s.units().iter().copied()) {
        match decoded {
            Ok(ch) => encoded.push(ch),
            Err(e) => encoded.push_str(&format!("\\u{:04x}", e.unpaired_surrogate())),
        }
    }
    JsonParser::new(&encoded).parse()
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

    fn push_str(&mut self, out: &mut crate::JsString, value: &str) -> Result<(), VmErr> {
        if out.len().saturating_add(value.len()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        out.push_str(value);
        Ok(())
    }

    fn push_char(&mut self, out: &mut crate::JsString, value: char) -> Result<(), VmErr> {
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

    fn string(&mut self) -> Result<crate::JsString, VmErr> {
        self.pos += 1; // opening quote
        let mut out = crate::JsString::default();
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
                            out.push_str(crate::JsString::from_units(vec![hi as u16]));
                            continue;
                        }
                        _ => return Err(VmErr::Msg("Invalid JSON".to_string())),
                    }
                    self.pos += 1;
                }
                Some(c) if c < 0x20 => return Err(VmErr::Msg("SyntaxError: Invalid JSON".into())),
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
                    if run.chars().any(|c| (c as u32) < 0x20) {
                        return Err(VmErr::Msg("SyntaxError: Invalid JSON".into()));
                    }
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
            props.push((key.to_key(), v));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindent_nests_objects_and_arrays() {
        assert_eq!(reindent(r#"{"a":1}"#, "  ").unwrap(), "{\n  \"a\": 1\n}");
        assert_eq!(
            reindent(r#"{"a":[1,2]}"#, "  ").unwrap(),
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}"
        );
    }

    #[test]
    fn reindent_keeps_empty_containers_on_one_line() {
        assert_eq!(reindent("{}", "  ").unwrap(), "{}");
        assert_eq!(reindent("[]", "  ").unwrap(), "[]");
        assert_eq!(
            reindent(r#"{"a":{},"b":[]}"#, "  ").unwrap(),
            "{\n  \"a\": {},\n  \"b\": []\n}"
        );
    }

    #[test]
    fn reindent_ignores_structure_inside_strings() {
        assert_eq!(
            reindent(r#"{"a":"{x},[y]"}"#, "  ").unwrap(),
            "{\n  \"a\": \"{x},[y]\"\n}"
        );
        // Trailing spaces inside a string must not read as an empty body.
        assert_eq!(
            reindent(r#"{"a":"  "}"#, "  ").unwrap(),
            "{\n  \"a\": \"  \"\n}"
        );
    }

    #[test]
    fn reindent_accepts_string_indent() {
        assert_eq!(reindent(r#"{"a":1}"#, "\t").unwrap(), "{\n\t\"a\": 1\n}");
    }
}
