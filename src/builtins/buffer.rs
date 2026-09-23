//! Node-compatible Buffer values over the VM's shared Uint8Array storage.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{Buffer, BufferBacking, PropAttrs, TypedArrayData, TypedKind, Value};

const MAX_BUFFER_LENGTH: usize = crate::value::MAX_STRING_LEN;

pub(super) fn install(environment: &mut Environment) {
    let Some(constructor) = environment.get("Buffer") else {
        return;
    };
    let Some(uint8_constructor) = environment.get("Uint8Array") else {
        return;
    };
    let Some(uint8_prototype) = uint8_constructor.get_prop("prototype") else {
        return;
    };

    super::make_callable(&constructor, buffer_constructor, Some(buffer_constructor));
    for (name, function) in [
        ("alloc", buffer_alloc as super::NativeFn),
        ("allocUnsafe", buffer_alloc),
        ("from", buffer_from),
        ("concat", buffer_concat),
        ("isBuffer", buffer_is_buffer),
        ("byteLength", buffer_byte_length),
        ("isEncoding", buffer_is_encoding),
    ] {
        constructor
            .set_prop(name.into(), super::nf(name, function))
            .expect("Buffer static method");
    }
    constructor
        .set_prop("name".into(), Value::String("Buffer".into()))
        .expect("Buffer constructor name");
    if let Value::Object { props } = &constructor {
        let mut metadata = props.meta.borrow_mut();
        metadata.set_attrs(
            "name",
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
        for name in [
            "alloc",
            "allocUnsafe",
            "from",
            "concat",
            "isBuffer",
            "byteLength",
            "isEncoding",
        ] {
            metadata.set_attrs(
                name,
                PropAttrs {
                    enumerable: false,
                    ..PropAttrs::default()
                },
            );
        }
    }

    let prototype = Value::object_with_proto(
        vec![
            ("constructor".into(), constructor.clone()),
            ("toString".into(), super::nf("toString", buffer_to_string)),
            ("slice".into(), super::nf("slice", buffer_slice)),
            ("write".into(), super::nf("write", buffer_write)),
            ("equals".into(), super::nf("equals", buffer_equals)),
            ("toJSON".into(), super::nf("toJSON", buffer_to_json)),
        ],
        Some(Rc::new(uint8_prototype)),
    );
    if let Value::Object { props } = &prototype {
        let mut metadata = props.meta.borrow_mut();
        for name in [
            "constructor",
            "toString",
            "slice",
            "write",
            "equals",
            "toJSON",
        ] {
            metadata.set_attrs(
                name,
                PropAttrs {
                    enumerable: false,
                    ..PropAttrs::default()
                },
            );
        }
    }
    super::set_builtin_constructor_prototype(environment, &constructor, prototype);
    if let Value::Object { props } = &constructor {
        props.set_proto(Some(Rc::new(uint8_constructor)));
    }
}

fn byte_value(value: &Value) -> Result<u8, VmErr> {
    if matches!(value, Value::Symbol(_)) {
        return Err(type_error("Cannot convert a Symbol value to a number"));
    }
    let number = value.to_number();
    if !number.is_finite() {
        return Ok(0);
    }
    Ok(number.trunc().rem_euclid(256.0) as u8)
}

fn checked_length(value: &Value) -> Result<usize, VmErr> {
    let number = value.to_number();
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(range_error("The value of \"size\" is out of range"));
    }
    if number > MAX_BUFFER_LENGTH as f64 {
        return Err(range_error("The value of \"size\" is out of range"));
    }
    Ok(number as usize)
}

fn make_buffer(bytes: Vec<u8>) -> Value {
    let length = bytes.len();
    Value::TypedArray(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer: Buffer::owned(bytes).into(),
        byte_offset: 0,
        length,
        is_buffer: true,
    }))
}

fn make_buffer_view(buffer: BufferBacking, byte_offset: usize, length: usize) -> Value {
    Value::TypedArray(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer,
        byte_offset,
        length,
        is_buffer: true,
    }))
}

fn buffer_view(value: &Value) -> Result<Rc<TypedArrayData>, VmErr> {
    match value {
        Value::TypedArray(view) if view.kind == TypedKind::Uint8 => Ok(view.clone()),
        _ => Err(type_error(
            "The \"this\" value must be a Uint8Array or Buffer",
        )),
    }
}

fn visible_bytes(view: &TypedArrayData) -> Result<Vec<u8>, VmErr> {
    let length = view.effective_length();
    view.buffer
        .read(view.effective_byte_offset(), length)
        .ok_or_else(|| range_error("Buffer range is outside its backing store"))
}

fn buffer_from(interpreter: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let source = args.first().cloned().unwrap_or(Value::Undefined);
    match &source {
        Value::String(string) => {
            let encoding = encoding_arg(args.get(1))?;
            Ok(make_buffer(encode(string, &encoding)?))
        }
        Value::ArrayBuffer(buffer) => {
            if buffer.is_detached() {
                return Err(type_error(
                    "Cannot create a Buffer from a detached ArrayBuffer",
                ));
            }
            let backing = BufferBacking::Array(buffer.clone());
            let available = backing.len();
            let offset = args
                .get(1)
                .map(checked_length_value)
                .transpose()?
                .unwrap_or(0);
            let length = args
                .get(2)
                .map(checked_length_value)
                .transpose()?
                .unwrap_or_else(|| available.saturating_sub(offset));
            if offset > available || length > available - offset {
                return Err(range_error("Buffer byte range is outside the ArrayBuffer"));
            }
            Ok(make_buffer_view(backing, offset, length))
        }
        Value::SharedArrayBuffer(buffer) => {
            let backing = BufferBacking::Shared(buffer.clone());
            let available = backing.len();
            let offset = args
                .get(1)
                .map(checked_length_value)
                .transpose()?
                .unwrap_or(0);
            let length = args
                .get(2)
                .map(checked_length_value)
                .transpose()?
                .unwrap_or_else(|| available.saturating_sub(offset));
            if offset > available || length > available - offset {
                return Err(range_error(
                    "Buffer byte range is outside the SharedArrayBuffer",
                ));
            }
            Ok(make_buffer_view(backing, offset, length))
        }
        Value::TypedArray(view) if view.kind == TypedKind::Uint8 => {
            Ok(make_buffer(visible_bytes(view)?))
        }
        Value::TypedArray(view) => {
            let mut bytes = Vec::with_capacity(view.effective_length());
            for index in 0..view.effective_length() {
                let element = super::read_element(view, index).unwrap_or(Value::Undefined);
                bytes.push(byte_value(&element)?);
            }
            Ok(make_buffer(bytes))
        }
        Value::Array(array) => {
            let items = array.borrow().clone();
            items
                .iter()
                .map(byte_value)
                .collect::<Result<Vec<_>, _>>()
                .map(make_buffer)
        }
        Value::Object { .. } => {
            let json_data = interpreter
                .prop(&source, &Value::String("data".into()))
                .ok();
            let json_buffer = matches!(
                interpreter
                    .prop(&source, &Value::String("type".into()))
                    .ok(),
                Some(Value::String(ref kind)) if kind == "Buffer"
            );
            let items = if json_buffer {
                if let Some(Value::Array(data)) = json_data.as_ref() {
                    data.borrow().clone()
                } else {
                    return Err(type_error("The Buffer JSON data property must be an array"));
                }
            } else if let Ok(Value::Number(length)) =
                interpreter.prop(&source, &Value::String("length".into()))
            {
                let length = checked_length(&Value::Number(length))?;
                (0..length)
                    .map(|index| interpreter.prop(&source, &Value::String(index.to_string())))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                interpreter.iterate(&source)?
            };
            items
                .iter()
                .map(byte_value)
                .collect::<Result<Vec<_>, _>>()
                .map(make_buffer)
        }
        Value::Null | Value::Undefined => Err(type_error(
            "The first argument must be a string, Buffer, ArrayBuffer, Array, or array-like object",
        )),
        _ => Err(type_error(
            "The first argument must be a string or a supported byte source",
        )),
    }
}

fn buffer_constructor(
    interpreter: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if matches!(args.first(), Some(Value::Number(_))) {
        buffer_alloc(interpreter, this, args)
    } else {
        buffer_from(interpreter, this, args)
    }
}

fn checked_length_value(value: &Value) -> Result<usize, VmErr> {
    checked_length(value)
}

fn buffer_alloc(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let length = checked_length(args.first().unwrap_or(&Value::Undefined))?;
    let mut bytes = vec![0; length];
    if let Some(fill) = args
        .get(1)
        .filter(|value| !matches!(value, Value::Undefined))
    {
        let pattern = match fill {
            Value::String(string) => encode(string, &encoding_arg(args.get(2))?)?,
            Value::TypedArray(view) => visible_bytes(view)?,
            Value::Array(array) => array
                .borrow()
                .iter()
                .map(byte_value)
                .collect::<Result<Vec<_>, _>>()?,
            other => vec![byte_value(other)?],
        };
        if pattern.is_empty() && length > 0 {
            return Err(type_error("The argument 'fill' must not be empty"));
        }
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = pattern[index % pattern.len()];
        }
    }
    Ok(make_buffer(bytes))
}

fn buffer_concat(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Some(Value::Array(list)) = args.first() else {
        return Err(type_error("The first argument must be an array of Buffers"));
    };
    let list = list.borrow().clone();
    let total = list.iter().try_fold(0usize, |sum, value| {
        let Value::TypedArray(view) = value else {
            return Err(type_error("The list argument must contain only Buffers"));
        };
        sum.checked_add(view.effective_length())
            .ok_or_else(|| range_error("Buffer length overflow"))
    })?;
    let length = args
        .get(1)
        .filter(|value| !matches!(value, Value::Undefined))
        .map(checked_length)
        .transpose()?
        .unwrap_or(total);
    if length > MAX_BUFFER_LENGTH {
        return Err(range_error("The value of \"totalLength\" is out of range"));
    }
    let mut bytes = vec![0; length];
    let mut cursor = 0;
    for value in list {
        let Value::TypedArray(view) = &value else {
            unreachable!("the list was checked above")
        };
        let copied = visible_bytes(view)?;
        let count = copied.len().min(length.saturating_sub(cursor));
        bytes[cursor..cursor + count].copy_from_slice(&copied[..count]);
        cursor += count;
        if cursor == length {
            break;
        }
    }
    Ok(make_buffer(bytes))
}

fn buffer_is_buffer(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(
        matches!(args.first(), Some(Value::TypedArray(view)) if view.is_buffer),
    ))
}

fn buffer_byte_length(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let source = args.first().cloned().unwrap_or(Value::Undefined);
    let length = match &source {
        Value::String(string) => encode(string, &encoding_arg(args.get(1))?)?.len(),
        Value::ArrayBuffer(buffer) => buffer.borrow().len(),
        Value::SharedArrayBuffer(buffer) => buffer.len(),
        Value::TypedArray(view) => view.effective_length() * view.kind.size(),
        _ => {
            return Err(type_error(
                "The first argument must be a string or buffer-like value",
            ));
        }
    };
    Ok(Value::Number(length as f64))
}

fn buffer_is_encoding(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Some(Value::String(name)) = args.first() else {
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(normalize_encoding(name).is_some()))
}

fn buffer_to_string(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let view = buffer_view(&this)?;
    let encoding = encoding_arg(args.first())?;
    let length = view.effective_length();
    let start = args.get(1).map(|value| index(value, length)).unwrap_or(0);
    let end = args
        .get(2)
        .map(|value| index(value, length))
        .unwrap_or(length);
    let end = end.max(start);
    let bytes = view
        .buffer
        .read(
            view.effective_byte_offset() + start,
            end.saturating_sub(start),
        )
        .ok_or_else(|| range_error("Buffer range is outside its backing store"))?;
    Value::checked_string(decode(&bytes, &encoding)).map_err(|_| range_error("String is too long"))
}

fn buffer_slice(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let view = buffer_view(&this)?;
    let length = view.effective_length();
    let start = args.first().map(|value| index(value, length)).unwrap_or(0);
    let end = args
        .get(1)
        .map(|value| index(value, length))
        .unwrap_or(length);
    let end = end.max(start);
    Ok(make_buffer_view(
        view.buffer.clone(),
        view.effective_byte_offset() + start,
        end - start,
    ))
}

fn buffer_write(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let view = buffer_view(&this)?;
    let Some(Value::String(string)) = args.first() else {
        return Err(type_error("The first argument must be a string"));
    };
    let (offset, length, encoding) = if args
        .get(1)
        .is_some_and(|value| matches!(value, Value::String(_)))
    {
        if args.len() > 2 {
            return Err(type_error("The \"offset\" argument must be of type number"));
        }
        let encoding = encoding_arg(args.get(1))?;
        (0, view.effective_length(), encoding)
    } else {
        let offset = args
            .get(1)
            .map(|value| index(value, view.effective_length()))
            .unwrap_or(0);
        let length = args
            .get(2)
            .map(|value| index(value, view.effective_length()))
            .unwrap_or_else(|| view.effective_length().saturating_sub(offset));
        let encoding = encoding_arg(args.get(3))?;
        (offset, length, encoding)
    };
    let bytes = encode(string, &encoding)?;
    let count = bytes
        .len()
        .min(length)
        .min(view.effective_length().saturating_sub(offset));
    view.buffer
        .write(view.effective_byte_offset() + offset, &bytes[..count]);
    Ok(Value::Number(count as f64))
}

fn buffer_equals(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let left = buffer_view(&this)?;
    let Some(Value::TypedArray(right)) = args.first() else {
        return Err(type_error("The argument must be a Buffer or Uint8Array"));
    };
    Ok(Value::Bool(visible_bytes(&left)? == visible_bytes(right)?))
}

fn buffer_to_json(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let view = buffer_view(&this)?;
    let bytes = visible_bytes(&view)?
        .into_iter()
        .map(|byte| Value::Number(byte as f64))
        .collect();
    Ok(Value::object(vec![
        ("type".into(), Value::String("Buffer".into())),
        ("data".into(), Value::array(bytes)),
    ]))
}

fn encoding_arg(value: Option<&Value>) -> Result<String, VmErr> {
    match value {
        None | Some(Value::Undefined) => Ok("utf8".into()),
        Some(Value::String(name)) => normalize_encoding(name)
            .map(str::to_string)
            .ok_or_else(|| type_error("Unknown encoding")),
        Some(_) => Err(type_error("The encoding argument must be a string")),
    }
}

fn normalize_encoding(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "utf8" | "utf-8" => Some("utf8"),
        "hex" => Some("hex"),
        "ascii" => Some("ascii"),
        "latin1" | "binary" => Some("latin1"),
        "base64" => Some("base64"),
        "base64url" => Some("base64url"),
        "ucs2" | "ucs-2" | "utf16le" | "utf-16le" => Some("utf16le"),
        _ => None,
    }
}

fn encode(string: &str, encoding: &str) -> Result<Vec<u8>, VmErr> {
    let encoding = normalize_encoding(encoding).ok_or_else(|| type_error("Unknown encoding"))?;
    let bytes = match encoding {
        "utf8" => string.as_bytes().to_vec(),
        "hex" => {
            let mut bytes = Vec::new();
            for pair in string.as_bytes().chunks_exact(2) {
                let (Some(high), Some(low)) = (hex_nibble(pair[0]), hex_nibble(pair[1])) else {
                    break;
                };
                bytes.push((high << 4) | low);
            }
            bytes
        }
        "ascii" | "latin1" => string.chars().map(|ch| ch as u32 as u8).collect(),
        "utf16le" => string.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        "base64" | "base64url" => decode_base64(string),
        _ => unreachable!(),
    };
    if bytes.len() > MAX_BUFFER_LENGTH {
        return Err(range_error("The value of \"size\" is out of range"));
    }
    Ok(bytes)
}

fn decode(bytes: &[u8], encoding: &str) -> String {
    match encoding {
        "utf8" => String::from_utf8_lossy(bytes).into_owned(),
        "hex" => bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        "ascii" => bytes.iter().map(|byte| char::from(byte & 0x7f)).collect(),
        "latin1" => bytes.iter().map(|byte| char::from(*byte)).collect(),
        "utf16le" => {
            let units = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        }
        "base64" => encode_base64(bytes, false),
        "base64url" => encode_base64(bytes, true),
        _ => String::new(),
    }
}

fn encode_base64(bytes: &[u8], url_safe: bool) -> String {
    const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let alphabet = if url_safe { URL_SAFE } else { STANDARD };
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied();
        let third = chunk.get(2).copied();
        output.push(alphabet[(first >> 2) as usize] as char);
        output.push(alphabet[(((first & 0x03) << 4) | second.unwrap_or(0) >> 4) as usize] as char);
        if let Some(second) = second {
            output.push(
                alphabet[(((second & 0x0f) << 2) | third.unwrap_or(0) >> 6) as usize] as char,
            );
        } else if !url_safe {
            output.push('=');
        }
        if let Some(third) = third {
            output.push(alphabet[(third & 0x3f) as usize] as char);
        } else if !url_safe {
            output.push('=');
        }
    }
    output
}

fn decode_base64(input: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    output
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn index(value: &Value, length: usize) -> usize {
    let number = value.to_number();
    if number.is_nan() || number <= 0.0 {
        return if number < 0.0 {
            (length as f64 + number.trunc()).max(0.0) as usize
        } else {
            0
        };
    }
    if !number.is_finite() {
        return length;
    }
    (number.trunc() as usize).min(length)
}

fn type_error(message: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {message}"))
}

fn range_error(message: &str) -> VmErr {
    VmErr::Msg(format!("RangeError: {message}"))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use crate::interpreter::Interpreter;
    use crate::value::Value;

    #[test]
    fn buffer_fixture_matches_node_and_bun() {
        let fixture = r#"(() => {
  const hex = Buffer.from('6869', 'hex');
  const base64 = Buffer.from('aGk=', 'base64');
  const storage = new ArrayBuffer(4);
  new Uint8Array(storage).set([1, 2, 3, 4]);
  const view = Buffer.from(storage, 1, 2);
  view.slice(0, 1)[0] = 9;
  const written = Buffer.alloc(4).write('hey', 1, 2, 'utf8');
  const legacyWritten = Buffer.alloc(4);
  const legacyWriteLength = legacyWritten.write('6869', 'hex');
  const constructed = new Buffer(2);
  const data = Buffer.from({type: 'Buffer', data: [97, 98]});
  const repeated = Buffer.alloc(3, 'ab');
  const joined = Buffer.concat([hex, Buffer.from('!')]);
  const originalToJSON = Buffer.prototype.toJSON;
  Buffer.prototype.toJSON = 1;
  const overriddenToJSON = JSON.stringify(hex);
  Buffer.prototype.toJSON = originalToJSON;
  return JSON.stringify({
    isBuffer: Buffer.isBuffer(hex),
    notTypedArrayBuffer: Buffer.isBuffer(new Uint8Array(2)) === false,
    prototype: Object.getPrototypeOf(hex) === Buffer.prototype,
    prototypeParent: Object.getPrototypeOf(Buffer.prototype) === Uint8Array.prototype,
    constructorParent: Object.getPrototypeOf(Buffer) === Uint8Array,
    uint8Instance: hex instanceof Uint8Array,
    defaultText: hex.toString(),
    hexText: hex.toString('hex'),
    base64Text: base64.toString(),
    aliasBytes: [storage.byteLength, new Uint8Array(storage)[1], new Uint8Array(storage)[2]],
    written,
    legacyWriteLength,
    legacyWrittenText: legacyWritten.toString('hex'),
    constructedLength: constructed.length,
    overriddenToJSON,
    writtenText: Buffer.alloc(4).write('hey', 1, 2, 'utf8'),
    dataText: data.toString(),
    repeated: repeated.toString(),
    joined: joined.toString(),
    byteLength: Buffer.byteLength('🌍'),
    json: JSON.stringify(hex),
    encoding: Buffer.isEncoding('base64url'),
    allocUnsafeLength: Buffer.allocUnsafe(3).length,
  });
})()"#;
        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter.eval_source(fixture).unwrap();
        let Value::String(ref result) = result else {
            panic!("Buffer fixture returned {result:?}");
        };
        let expected: serde_json::Value = serde_json::from_str(result).unwrap();
        assert_eq!(expected["isBuffer"], true);
        assert_eq!(expected["notTypedArrayBuffer"], true);
        assert_eq!(expected["prototype"], true);
        assert_eq!(expected["prototypeParent"], true);
        assert_eq!(expected["constructorParent"], true);
        assert_eq!(expected["uint8Instance"], true);
        assert_eq!(expected["defaultText"], "hi");
        assert_eq!(expected["hexText"], "6869");
        assert_eq!(expected["base64Text"], "hi");
        assert_eq!(expected["aliasBytes"], serde_json::json!([4, 9, 3]));
        assert_eq!(expected["written"], 2);
        assert_eq!(expected["legacyWriteLength"], 2);
        assert_eq!(expected["legacyWrittenText"], "68690000");
        assert_eq!(expected["constructedLength"], 2);
        assert_eq!(expected["overriddenToJSON"], r#"{"0":104,"1":105}"#);
        assert_eq!(expected["dataText"], "ab");
        assert_eq!(expected["repeated"], "aba");
        assert_eq!(expected["joined"], "hi!");
        assert_eq!(expected["byteLength"], 4);
        assert_eq!(expected["json"], r#"{"type":"Buffer","data":[104,105]}"#);
        assert_eq!(expected["encoding"], true);
        assert_eq!(expected["allocUnsafeLength"], 3);

        for runtime in ["node", "bun"] {
            let Ok(reference) = Command::new(runtime)
                .args(["-e", &format!("process.stdout.write({fixture})")])
                .output()
            else {
                continue;
            };
            if !reference.status.success() {
                eprintln!(
                    "skipping {runtime} Buffer comparison: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                continue;
            }
            let actual: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(expected, actual, "{runtime} Buffer behavior differed");
        }
    }
}
