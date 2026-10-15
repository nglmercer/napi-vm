//! ECMA-402 ListFormat. Guest iteration stays observable; ICU supplies patterns.
use super::*;
use icu_list::{
    ListFormatter,
    options::{ListFormatterOptions, ListLength},
};
use std::fmt;
use writeable::{Part, PartsWrite, Writeable};

#[derive(Clone)]
pub(crate) struct Slots {
    locale: String,
    kind: String,
    style: String,
    formatter: Rc<ListFormatter>,
}
fn error(message: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {message}"))
}
fn slot(this: &Value) -> Result<Slots, VmErr> {
    this.property_cell()
        .and_then(|p| p.meta.borrow().intl_list.clone())
        .ok_or_else(|| error("Intl.ListFormat receiver required"))
}
fn method(target: &Value, name: &str, length: usize, f: NativeFn, fp: &Option<Value>) {
    nonenum(target, name, native_method(name, length, f, fp.clone()));
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|o| o.get_prop("prototype"));
    let constructor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&constructor, super::super::require_new, Some(construct));
    for (key, value) in [
        ("name", Value::String("ListFormat".into())),
        ("length", Value::Number(0.)),
    ] {
        nonenum(&constructor, key, value);
        constructor
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_attrs(
                key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
    }
    method(
        &constructor,
        "supportedLocalesOf",
        1,
        supported_locales,
        &fp,
    );
    let prototype = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&prototype, "constructor", constructor.clone());
    tag(&prototype, "Intl.ListFormat");
    method(&prototype, "resolvedOptions", 0, resolved_options, &fp);
    method(&prototype, "format", 1, format, &fp);
    method(&prototype, "formatToParts", 1, format_to_parts, &fp);
    super::super::set_builtin_constructor_prototype(e, &constructor, prototype.clone());
    nonenum(namespace, "ListFormat", constructor.clone());
    e.install_intrinsic("Intl.ListFormat", constructor);
    e.install_intrinsic("Intl.ListFormat.prototype", prototype);
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let target = i
        .new_target_stack
        .last()
        .cloned()
        .ok_or_else(|| error("ListFormat requires new"))?;
    let prototype = i.constructor_prototype(&target, "Intl.ListFormat")?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        value if crate::interpreter::call::is_js_object(&value) => value,
        _ => return Err(error("ListFormat options must be an object")),
    };
    locale_matcher(i, &options)?;
    let locale = requested
        .iter()
        .find_map(|tag| best_available_locale(tag))
        .unwrap_or_else(|| "en-US".into());
    let kind = option(i, &options, "type")?.unwrap_or_else(|| "conjunction".into());
    if !["conjunction", "disjunction", "unit"].contains(&kind.as_str()) {
        return Err(VmErr::Msg("RangeError: Invalid list type".into()));
    }
    let style = option(i, &options, "style")?.unwrap_or_else(|| "long".into());
    let length = match style.as_str() {
        "long" => ListLength::Wide,
        "short" => ListLength::Short,
        "narrow" => ListLength::Narrow,
        _ => return Err(VmErr::Msg("RangeError: Invalid list style".into())),
    };
    let prefs = locale
        .parse::<icu_locale::Locale>()
        .unwrap_or_else(|_| "en-US".parse().unwrap())
        .into();
    let options = ListFormatterOptions::default().with_length(length);
    let formatter = match kind.as_str() {
        "conjunction" => ListFormatter::try_new_and(prefs, options),
        "disjunction" => ListFormatter::try_new_or(prefs, options),
        _ => ListFormatter::try_new_unit(prefs, options),
    }
    .map_err(|e| VmErr::Msg(format!("RangeError: {e}")))?;
    let result = Value::object_with_proto(vec![], prototype);
    result.property_cell().unwrap().meta.borrow_mut().intl_list = Some(Slots {
        locale,
        kind,
        style,
        formatter: Rc::new(formatter),
    });
    Ok(result)
}
fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    Ok(Value::object(vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("type".into(), Value::String(s.kind.into())),
        ("style".into(), Value::String(s.style.into())),
    ]))
}
fn string_list(i: &mut Interpreter, source: Value) -> Result<Vec<crate::JsString>, VmErr> {
    if matches!(source, Value::Undefined) {
        return Ok(Vec::new());
    }
    let iterator = i.iterator_for(&source)?;
    let next = i.member(&iterator, "next")?;
    let mut strings = Vec::new();
    loop {
        i.consume_loop()?;
        let result = i.call_this(&next, iterator.clone(), vec![])?;
        let (done, value) = i.iterator_result_fields(&result)?;
        if done {
            return Ok(strings);
        }
        if let Value::String(ref text) = value {
            strings.push(text.clone());
        } else {
            let _ = i.close_guest_iterator(&iterator, false);
            return Err(error("List elements must be strings"));
        }
        if strings.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
    }
}
struct Sink<'a> {
    elements: &'a [crate::JsString],
    output: Vec<(bool, crate::JsString)>,
    element: usize,
    inside_element: bool,
}
impl fmt::Write for Sink<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.inside_element || text.is_empty() {
            return Ok(());
        }
        if let Some((false, last)) = self.output.last_mut() {
            last.push_str(text);
        } else {
            self.output.push((false, text.into()));
        }
        Ok(())
    }
}
impl PartsWrite for Sink<'_> {
    type SubPartsWrite = Self;
    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        if part == icu_list::parts::ELEMENT {
            let text = self.elements.get(self.element).ok_or(fmt::Error)?.clone();
            self.element += 1;
            self.output.push((true, text));
            let old = self.inside_element;
            self.inside_element = true;
            let result = f(self);
            self.inside_element = old;
            result
        } else {
            f(self)
        }
    }
}
fn render(i: &mut Interpreter, this: Value, args: Vec<Value>, parts: bool) -> Result<Value, VmErr> {
    let slots = slot(&this)?;
    let elements = string_list(i, arg(&args, 0))?;
    // ICU sees UTF-8 for conditional pattern selection. Element spans in its
    // output are replaced with their original UTF-16 strings, losslessly.
    let strings: Vec<String> = elements.iter().map(ToString::to_string).collect();
    let mut sink = Sink {
        elements: &elements,
        output: Vec::new(),
        element: 0,
        inside_element: false,
    };
    slots
        .formatter
        .format(strings.iter())
        .write_to_parts(&mut sink)
        .map_err(|_| error("List formatting failed"))?;
    if parts {
        Value::checked_array(
            sink.output
                .into_iter()
                .map(|(element, text)| {
                    Value::object(vec![
                        (
                            "type".into(),
                            Value::String(if element { "element" } else { "literal" }.into()),
                        ),
                        ("value".into(), Value::String(text)),
                    ])
                })
                .collect(),
        )
    } else {
        let mut output = crate::JsString::default();
        for (_, text) in sink.output {
            output.push_str(text);
            if output.len() > crate::value::MAX_STRING_LEN {
                return Err(crate::value::limit_err("Maximum string length exceeded"));
            }
        }
        Value::checked_string(output)
    }
}
fn format(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    render(i, this, args, false)
}
fn format_to_parts(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    render(i, this, args, true)
}
