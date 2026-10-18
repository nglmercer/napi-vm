//! Generic RegExp protocol operations, including observable overrides of exec.
use super::*;

fn object(value: &Value) -> Result<(), VmErr> {
    if crate::interpreter::call::is_js_object(value) {
        Ok(())
    } else {
        Err(VmErr::Msg(
            "TypeError: RegExp method receiver must be an object".into(),
        ))
    }
}
fn length(i: &mut Interpreter, value: &Value) -> Result<usize, VmErr> {
    let number = i.ecmascript_to_number(value)?;
    Ok(if number.is_nan() || number < 0. {
        0
    } else {
        number.trunc().min(9_007_199_254_740_991.) as usize
    })
}
fn regexp_exec(i: &mut Interpreter, rx: &Value, input: &crate::JsString) -> Result<Value, VmErr> {
    let method = i.member(rx, "exec")?;
    if crate::interpreter::call::is_callable_value(&method) {
        let result = i.call_this(&method, rx.clone(), vec![Value::String(input.clone())])?;
        if matches!(result, Value::Null) || crate::interpreter::call::is_js_object(&result) {
            return Ok(result);
        }
        return Err(VmErr::Msg(
            "TypeError: RegExp exec must return an object or null".into(),
        ));
    }
    super::regexp_exec(i, rx.clone(), vec![Value::String(input.clone())])
}
pub(super) fn test(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let input = i.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    Ok(Value::Bool(!matches!(
        regexp_exec(i, &this, &input)?,
        Value::Null
    )))
}
pub(super) fn matches(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let input = i.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let flags = i.member(&this, "flags")?;
    let flags = i.ecmascript_to_string(&flags)?;
    if !flags.contains('g') {
        return regexp_exec(i, &this, &input);
    }
    let unicode = flags.contains('u') || flags.contains('v');
    i.set_member_str_or_throw(&this, "lastIndex", Value::Number(0.))?;
    let mut out = Vec::new();
    loop {
        i.consume_loop()?;
        let result = regexp_exec(i, &this, &input)?;
        if matches!(result, Value::Null) {
            return if out.is_empty() {
                Ok(Value::Null)
            } else {
                Value::checked_array(out)
            };
        }
        let matched = i.member(&result, "0")?;
        let matched = i.ecmascript_to_string(&matched)?;
        if matched.is_empty() {
            let index = i.member(&this, "lastIndex")?;
            let index = length(i, &index)?;
            i.set_member_str_or_throw(
                &this,
                "lastIndex",
                Value::Number(advance(&input, index, unicode) as f64),
            )?;
        }
        out.push(Value::String(matched));
        if out.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
    }
}
pub(super) fn search(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let input = i.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let previous = i.member(&this, "lastIndex")?;
    if !crate::builtins::object::same_value(&previous, &Value::Number(0.)) {
        i.set_member_str_or_throw(&this, "lastIndex", Value::Number(0.))?;
    }
    let result = regexp_exec(i, &this, &input)?;
    let current = i.member(&this, "lastIndex")?;
    if !crate::builtins::object::same_value(&current, &previous) {
        i.set_member_str_or_throw(&this, "lastIndex", previous)?;
    }
    if matches!(result, Value::Null) {
        Ok(Value::Number(-1.))
    } else {
        i.member(&result, "index")
    }
}
pub(super) fn flags(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let mut flags = String::new();
    for (name, flag) in [
        ("hasIndices", 'd'),
        ("global", 'g'),
        ("ignoreCase", 'i'),
        ("multiline", 'm'),
        ("dotAll", 's'),
        ("unicode", 'u'),
        ("unicodeSets", 'v'),
        ("sticky", 'y'),
    ] {
        if i.member(&this, name)?.is_truthy() {
            flags.push(flag);
        }
    }
    Ok(Value::String(flags.into()))
}
pub(super) fn to_string(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let source = i.member(&this, "source")?;
    let source = i.ecmascript_to_string(&source)?;
    let flags = i.member(&this, "flags")?;
    let flags = i.ecmascript_to_string(&flags)?;
    Ok(Value::String(
        crate::JsString::from("/")
            .concat(&source)
            .concat(&crate::JsString::from("/"))
            .concat(&flags),
    ))
}

pub(crate) struct IteratorSlots {
    regex: Value,
    input: crate::JsString,
    global: bool,
    unicode: bool,
    done: bool,
}
impl IteratorSlots {
    pub(crate) fn values(&self) -> Vec<Value> {
        vec![self.regex.clone()]
    }
}
pub(crate) fn install_iterator(e: &mut Environment) {
    let parent = e.intrinsic("%IteratorPrototype%").unwrap();
    let proto = Value::object_with_proto(vec![], Some(Rc::new(parent)));
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    proto
        .set_prop(
            "next".into(),
            super::super::native_method("next", 0, iterator_next, fp),
        )
        .unwrap();
    proto.property_cell().unwrap().meta.borrow_mut().set_attrs(
        "next",
        crate::value::PropAttrs {
            enumerable: false,
            ..Default::default()
        },
    );
    if let Some(Value::Symbol(ref symbol)) = super::super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        proto
            .set_prop(key.clone(), Value::String("RegExp String Iterator".into()))
            .unwrap();
        let cell = proto.property_cell().unwrap();
        let mut meta = cell.meta.borrow_mut();
        meta.set_symbol_key(&key, symbol.clone());
        meta.set_attrs(
            &key,
            crate::value::PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    e.install_intrinsic("%RegExpStringIteratorPrototype%", proto);
}
pub(super) fn match_all(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    object(&this)?;
    let input = i.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let constructor = i.species_constructor(&this, "RegExp")?;
    let flags = i.member(&this, "flags")?;
    let flags = i.ecmascript_to_string(&flags)?;
    let matcher = i.ctor(
        &constructor,
        vec![this.clone(), Value::String(flags.clone())],
    )?;
    let index = i.member(&this, "lastIndex")?;
    let index = length(i, &index)?;
    i.set_member_str_or_throw(&matcher, "lastIndex", Value::Number(index as f64))?;
    let proto = i
        .persistent_global
        .borrow()
        .intrinsic("%RegExpStringIteratorPrototype%")
        .unwrap();
    let result = Value::object_with_proto(vec![], Some(Rc::new(proto)));
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .regexp_iterator = Some(Rc::new(RefCell::new(IteratorSlots {
        regex: matcher,
        input,
        global: flags.contains('g'),
        unicode: flags.contains('u') || flags.contains('v'),
        done: false,
    })));
    Ok(result)
}
fn iterator_next(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let slots = this
        .property_cell()
        .and_then(|p| p.meta.borrow().regexp_iterator.clone())
        .ok_or_else(|| VmErr::Msg("TypeError: RegExp String Iterator receiver required".into()))?;
    let (rx, input, global, unicode, done) = {
        let s = slots.borrow();
        (
            s.regex.clone(),
            s.input.clone(),
            s.global,
            s.unicode,
            s.done,
        )
    };
    if done {
        return Ok(crate::interpreter::call::iter_result(
            Value::Undefined,
            true,
        ));
    }
    let result = regexp_exec(i, &rx, &input)?;
    if matches!(result, Value::Null) {
        slots.borrow_mut().done = true;
        return Ok(crate::interpreter::call::iter_result(
            Value::Undefined,
            true,
        ));
    }
    if global {
        let matched = i.member(&result, "0")?;
        if i.ecmascript_to_string(&matched)?.is_empty() {
            let index = i.member(&rx, "lastIndex")?;
            let index = length(i, &index)?;
            i.set_member_str_or_throw(
                &rx,
                "lastIndex",
                Value::Number(advance(&input, index, unicode) as f64),
            )?;
        }
    } else {
        slots.borrow_mut().done = true;
    }
    Ok(crate::interpreter::call::iter_result(result, false))
}
pub(super) fn split(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let input = i.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let constructor = i.species_constructor(&this, "RegExp")?;
    let flags = i.member(&this, "flags")?;
    let flags = i.ecmascript_to_string(&flags)?;
    let unicode = flags.contains('u') || flags.contains('v');
    let flags = if flags.contains('y') {
        flags
    } else {
        flags.concat(&crate::JsString::from("y"))
    };
    let splitter = i.ctor(&constructor, vec![this, Value::String(flags)])?;
    let limit = match args.get(1) {
        None | Some(Value::Undefined) => u32::MAX as usize,
        Some(v) => (crate::value::to_int32(i.ecmascript_to_number(v)?) as u32) as usize,
    };
    let mut out = Vec::new();
    if limit == 0 {
        return Ok(Value::array(out));
    }
    if input.is_empty() {
        if matches!(regexp_exec(i, &splitter, &input)?, Value::Null) {
            out.push(Value::String(input));
        }
        return Ok(Value::array(out));
    }
    let (mut p, mut q) = (0, 0);
    while q < input.len() {
        i.consume_loop()?;
        i.set_member_str_or_throw(&splitter, "lastIndex", Value::Number(q as f64))?;
        let result = regexp_exec(i, &splitter, &input)?;
        if matches!(result, Value::Null) {
            q = advance(&input, q, unicode);
            continue;
        }
        let end = i.member(&splitter, "lastIndex")?;
        let end = length(i, &end)?.min(input.len());
        if end == p {
            q = advance(&input, q, unicode);
            continue;
        }
        out.push(Value::String(input.slice(p, q)));
        if out.len() == limit {
            return Value::checked_array(out);
        }
        p = end;
        let n = i.member(&result, "length")?;
        let n = length(i, &n)?;
        for index in 1..n {
            i.consume_loop()?;
            out.push(i.member(&result, &index.to_string())?);
            if out.len() == limit {
                return Value::checked_array(out);
            }
            if out.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
        }
        q = p;
    }
    out.push(Value::String(input.slice(p, input.len())));
    Value::checked_array(out)
}

pub(super) fn replace(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    object(&this)?;
    let input = i.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let replacement = args.get(1).cloned().unwrap_or(Value::Undefined);
    let functional = crate::interpreter::call::is_callable_value(&replacement);
    let template = if functional {
        crate::JsString::default()
    } else {
        i.ecmascript_to_string(&replacement)?
    };
    let flags = i.member(&this, "flags")?;
    let flags = i.ecmascript_to_string(&flags)?;
    let global = flags.contains('g');
    let unicode = flags.contains('u') || flags.contains('v');
    if global {
        i.set_member_str_or_throw(&this, "lastIndex", Value::Number(0.))?;
    }
    let mut results = Vec::new();
    loop {
        i.consume_loop()?;
        let result = regexp_exec(i, &this, &input)?;
        if matches!(result, Value::Null) {
            break;
        }
        if global {
            let matched = i.member(&result, "0")?;
            if i.ecmascript_to_string(&matched)?.is_empty() {
                let index = i.member(&this, "lastIndex")?;
                let index = length(i, &index)?;
                i.set_member_str_or_throw(
                    &this,
                    "lastIndex",
                    Value::Number(advance(&input, index, unicode) as f64),
                )?;
            }
        }
        results.push(result);
        if !global {
            break;
        }
        if results.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
    }
    let mut out = crate::JsString::default();
    let mut cursor = 0;
    for result in results {
        let count = i.member(&result, "length")?;
        let count = length(i, &count)?.saturating_sub(1);
        let matched = i.member(&result, "0")?;
        let matched = i.ecmascript_to_string(&matched)?;
        let position = i.member(&result, "index")?;
        let position = length(i, &position)?.min(input.len());
        let mut captures = Vec::new();
        for index in 1..=count {
            i.consume_loop()?;
            let value = i.member(&result, &index.to_string())?;
            captures.push(if matches!(value, Value::Undefined) {
                Value::Undefined
            } else {
                Value::String(i.ecmascript_to_string(&value)?)
            });
            if captures.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
        }
        let groups = i.member(&result, "groups")?;
        let piece = if functional {
            let mut args = vec![Value::String(matched.clone())];
            args.extend(captures.iter().cloned());
            args.push(Value::Number(position as f64));
            args.push(Value::String(input.clone()));
            if !matches!(groups, Value::Undefined) {
                args.push(groups);
            }
            let value = i.call_this(&replacement, Value::Undefined, args)?;
            i.ecmascript_to_string(&value)?
        } else {
            let groups = if matches!(groups, Value::Undefined) {
                groups
            } else {
                super::super::object::to_object_receiver(&groups)?
            };
            substitute(i, &template, &matched, &input, position, &captures, &groups)?
        };
        if position >= cursor {
            out.push_str(input.slice(cursor, position));
            out.push_str(piece);
            cursor = position.saturating_add(matched.len());
            if out.len() > crate::value::MAX_STRING_LEN {
                return Err(crate::value::limit_err("Maximum string length exceeded"));
            }
        }
    }
    if cursor < input.len() {
        out.push_str(input.slice(cursor, input.len()));
    }
    Value::checked_string(out)
}
fn substitute(
    i: &mut Interpreter,
    template: &crate::JsString,
    matched: &crate::JsString,
    input: &crate::JsString,
    position: usize,
    captures: &[Value],
    groups: &Value,
) -> Result<crate::JsString, VmErr> {
    let mut out = crate::JsString::default();
    let mut at = 0;
    while at < template.len() {
        if template.units()[at] != 36 || at + 1 == template.len() {
            out.push_str(template.slice(at, at + 1));
            at += 1;
            continue;
        }
        let code = template.units()[at + 1];
        match code {
            36 => out.push_str("$"),
            38 => out.push_str(matched),
            96 => out.push_str(input.slice(0, position)),
            39 => out.push_str(input.slice(
                position.saturating_add(matched.len()).min(input.len()),
                input.len(),
            )),
            48..=57 => {
                let mut n = (code - 48) as usize;
                let mut width = 2;
                if let Some(&next) = template.units().get(at + 2)
                    && (48..=57).contains(&next)
                {
                    let two = n * 10 + (next - 48) as usize;
                    if two > 0 && two <= captures.len() {
                        n = two;
                        width = 3;
                    }
                }
                if n == 0 || n > captures.len() {
                    out.push_str("$");
                    at += 1;
                    continue;
                }
                if let Value::String(s) = &captures[n - 1] {
                    out.push_str(s);
                }
                at += width;
                continue;
            }
            60 if !matches!(groups, Value::Undefined) => {
                if let Some(end) = template.units()[at + 2..].iter().position(|c| *c == 62) {
                    let end = at + 2 + end;
                    let key = template.slice(at + 2, end).to_key();
                    let value = i.member(groups, &key)?;
                    if !matches!(value, Value::Undefined) {
                        out.push_str(i.ecmascript_to_string(&value)?);
                    }
                    at = end + 1;
                    continue;
                }
                out.push_str("$");
                at += 1;
                continue;
            }
            _ => {
                out.push_str("$");
                at += 1;
                continue;
            }
        }
        at += 2;
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    Ok(out)
}
