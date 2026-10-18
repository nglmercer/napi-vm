//! Uint8Array hex/base64 codecs with prefix writes and exact UTF-16 read counts.
use super::*;
use base64::Engine;

pub(super) fn install(e: &Environment) {
    let c = e.get("Uint8Array").unwrap();
    let p = c.get_prop("prototype").unwrap();
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    for (target, name, length, f) in [
        (&c, "fromHex", 1, from_hex as super::super::NativeFn),
        (&c, "fromBase64", 1, from_base64),
        (&p, "toHex", 0, to_hex),
        (&p, "toBase64", 0, to_base64),
        (&p, "setFromHex", 1, set_hex),
        (&p, "setFromBase64", 1, set_base64),
    ] {
        target
            .set_prop(
                name.into(),
                super::super::native_method(name, length, f, fp.clone()),
            )
            .unwrap();
        target.property_cell().unwrap().meta.borrow_mut().set_attrs(
            name,
            crate::value::PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    }
}
fn type_error() -> VmErr {
    VmErr::Msg("TypeError: Invalid Uint8Array codec argument".into())
}
fn syntax_error() -> VmErr {
    VmErr::Msg("SyntaxError: Invalid encoded byte string".into())
}
fn string(args: &[Value]) -> Result<crate::JsString, VmErr> {
    match args.first() {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(type_error()),
    }
}
fn view(this: &Value, write: bool) -> Result<Rc<TypedArrayData>, VmErr> {
    let v = require(this)?;
    if v.kind != TypedKind::Uint8 {
        return Err(type_error());
    }
    if write {
        require_writable(&v.buffer)?;
    }
    Ok(v)
}
fn bytes(v: &TypedArrayData) -> Result<Vec<u8>, VmErr> {
    if v.is_out_of_bounds() {
        return Err(type_error());
    }
    v.buffer
        .read(v.byte_offset, v.effective_length())
        .ok_or_else(type_error)
}
fn options(value: Option<&Value>) -> Result<Value, VmErr> {
    match value {
        None | Some(Value::Undefined) => Ok(Value::object_with_proto(vec![], None)),
        Some(v) if crate::interpreter::call::is_js_object(v) => Ok(v.clone()),
        _ => Err(type_error()),
    }
}
fn choice(
    i: &mut Interpreter,
    o: &Value,
    key: &str,
    default: &str,
    allowed: &[&str],
) -> Result<String, VmErr> {
    let v = i.member(o, key)?;
    if matches!(v, Value::Undefined) {
        return Ok(default.into());
    }
    let Value::String(ref s) = v else {
        return Err(type_error());
    };
    let s = s.to_string();
    if allowed.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(type_error())
    }
}
fn decode_options(i: &mut Interpreter, args: &[Value]) -> Result<(bool, String), VmErr> {
    let o = options(args.get(1))?;
    let url = choice(i, &o, "alphabet", "base64", &["base64", "base64url"])? == "base64url";
    let mode = choice(
        i,
        &o,
        "lastChunkHandling",
        "loose",
        &["loose", "strict", "stop-before-partial"],
    )?;
    Ok((url, mode))
}
struct Decoded {
    read: usize,
    bytes: Vec<u8>,
    error: bool,
}
fn hex(s: &crate::JsString, max: usize) -> Decoded {
    let mut result = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: !s.len().is_multiple_of(2),
    };
    if result.error {
        return result;
    }
    let digit = |u: u16| match u {
        48..=57 => Some((u - 48) as u8),
        65..=70 => Some((u - 55) as u8),
        97..=102 => Some((u - 87) as u8),
        _ => None,
    };
    while result.read < s.len() && result.bytes.len() < max {
        let a = digit(s.units()[result.read]);
        let b = digit(s.units()[result.read + 1]);
        match (a, b) {
            (Some(a), Some(b)) => result.bytes.push(a * 16 + b),
            _ => {
                result.error = true;
                break;
            }
        }
        result.read += 2;
    }
    result
}
fn whitespace(u: u16) -> bool {
    matches!(u, 9 | 10 | 12 | 13 | 32)
}
fn skip(s: &[u16], mut index: usize) -> usize {
    while s.get(index).is_some_and(|u| whitespace(*u)) {
        index += 1;
    }
    index
}
fn sextet(u: u16, url: bool) -> Option<u8> {
    match u {
        65..=90 => Some((u - 65) as u8),
        97..=122 => Some((u - 71) as u8),
        48..=57 => Some((u + 4) as u8),
        43 if !url => Some(62),
        47 if !url => Some(63),
        45 if url => Some(62),
        95 if url => Some(63),
        _ => None,
    }
}
fn append_final(result: &mut Decoded, chunk: &[u8], strict: bool) {
    if chunk.len() == 2 {
        if strict && chunk[1] & 15 != 0 {
            result.error = true;
            return;
        }
        result.bytes.push(chunk[0] << 2 | chunk[1] >> 4);
    } else {
        if strict && chunk[2] & 3 != 0 {
            result.error = true;
            return;
        }
        result
            .bytes
            .extend([chunk[0] << 2 | chunk[1] >> 4, chunk[1] << 4 | chunk[2] >> 2]);
    }
}
fn base64(s: &crate::JsString, url: bool, mode: &str, max: usize) -> Decoded {
    let mut r = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: false,
    };
    if max == 0 {
        return r;
    }
    let units = s.units();
    let mut chunk = Vec::with_capacity(4);
    let mut index = 0;
    loop {
        index = skip(units, index);
        if index == units.len() {
            if !chunk.is_empty() {
                if mode == "stop-before-partial" {
                    return r;
                }
                if mode == "strict" || chunk.len() == 1 {
                    r.error = true;
                    return r;
                }
                append_final(&mut r, &chunk, false);
            }
            r.read = units.len();
            return r;
        }
        let u = units[index];
        index += 1;
        if u == 61 {
            if chunk.len() < 2 {
                r.error = true;
                return r;
            }
            index = skip(units, index);
            if chunk.len() == 2 {
                if index == units.len() {
                    if mode != "stop-before-partial" {
                        r.error = true;
                    }
                    return r;
                }
                if units[index] == 61 {
                    index = skip(units, index + 1);
                }
            }
            if index < units.len() {
                r.error = true;
                return r;
            }
            append_final(&mut r, &chunk, mode == "strict");
            if !r.error {
                r.read = units.len();
            }
            return r;
        }
        let Some(value) = sextet(u, url) else {
            r.error = true;
            return r;
        };
        let remaining = max - r.bytes.len();
        if remaining == 1 && chunk.len() == 2 || remaining == 2 && chunk.len() == 3 {
            return r;
        }
        chunk.push(value);
        if chunk.len() == 4 {
            r.bytes.extend([
                chunk[0] << 2 | chunk[1] >> 4,
                chunk[1] << 4 | chunk[2] >> 2,
                chunk[2] << 6 | chunk[3],
            ]);
            chunk.clear();
            r.read = index;
            if r.bytes.len() == max {
                return r;
            }
        }
    }
}
fn allocated(i: &mut Interpreter, result: Decoded) -> Result<Value, VmErr> {
    if result.error {
        return Err(syntax_error());
    }
    let value = allocate_typed(TypedKind::Uint8, result.bytes.len())?;
    let Value::TypedArray(ref v) = value else {
        unreachable!()
    };
    v.buffer.write(0, &result.bytes);
    let proto = i
        .persistent_global
        .borrow()
        .intrinsic("Uint8Array")
        .and_then(|c| c.get_prop("prototype"));
    v.properties.set_proto(proto.map(Rc::new));
    Ok(value)
}
fn written(v: &TypedArrayData, result: Decoded) -> Result<Value, VmErr> {
    v.buffer.write(v.byte_offset, &result.bytes);
    if result.error {
        return Err(syntax_error());
    }
    Ok(Value::object(vec![
        ("read".into(), Value::Number(result.read as f64)),
        ("written".into(), Value::Number(result.bytes.len() as f64)),
    ]))
}
fn from_hex(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = string(&args)?;
    allocated(i, hex(&s, usize::MAX))
}
fn from_base64(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = string(&args)?;
    let (url, mode) = decode_options(i, &args)?;
    let result = base64(&s, url, &mode, usize::MAX);
    allocated(i, result)
}
fn set_hex(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let v = view(&this, true)?;
    let s = string(&args)?;
    if v.is_out_of_bounds() {
        return Err(type_error());
    }
    written(&v, hex(&s, v.effective_length()))
}
fn set_base64(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let v = view(&this, true)?;
    let s = string(&args)?;
    let (url, mode) = decode_options(i, &args)?;
    if v.is_out_of_bounds() {
        return Err(type_error());
    }
    written(&v, base64(&s, url, &mode, v.effective_length()))
}
fn to_hex(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let v = view(&this, false)?;
    let bytes = bytes(&v)?;
    let alphabet = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(alphabet[(byte >> 4) as usize] as char);
        result.push(alphabet[(byte & 15) as usize] as char);
    }
    Value::checked_string(result)
}
fn to_base64(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let v = view(&this, false)?;
    let o = options(args.first())?;
    let url = choice(i, &o, "alphabet", "base64", &["base64", "base64url"])? == "base64url";
    let omit = i.member(&o, "omitPadding")?.is_truthy();
    let bytes = bytes(&v)?;
    let engine = match (url, omit) {
        (false, false) => base64::engine::general_purpose::STANDARD,
        (false, true) => base64::engine::general_purpose::STANDARD_NO_PAD,
        (true, false) => base64::engine::general_purpose::URL_SAFE,
        (true, true) => base64::engine::general_purpose::URL_SAFE_NO_PAD,
    };
    Value::checked_string(engine.encode(bytes))
}
