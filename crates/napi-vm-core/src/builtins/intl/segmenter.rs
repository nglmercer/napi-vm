//! ECMA-402 Segmenter objects over ICU4X UTF-16 break iterators.
use super::*;
use icu_segmenter::{GraphemeClusterSegmenter, SentenceSegmenter, WordSegmenter};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Granularity {
    Grapheme,
    Word,
    Sentence,
}
impl Granularity {
    fn name(self) -> &'static str {
        match self {
            Self::Grapheme => "grapheme",
            Self::Word => "word",
            Self::Sentence => "sentence",
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct SegmenterSlots {
    locale: String,
    granularity: Granularity,
}
#[derive(Debug)]
struct Boundary {
    start: usize,
    end: usize,
    word_like: bool,
}
#[derive(Debug)]
pub(crate) struct SegmentsSlots {
    input: crate::JsString,
    boundaries: Vec<Boundary>,
    word: bool,
}
#[derive(Debug)]
pub(crate) struct IteratorSlots {
    segments: Rc<SegmentsSlots>,
    cursor: Cell<usize>,
}
fn brand_error() -> VmErr {
    VmErr::Msg("TypeError: incompatible Intl.Segmenter receiver".into())
}
fn segmenter(this: &Value) -> Result<SegmenterSlots, VmErr> {
    this.property_cell()
        .and_then(|p| p.meta.borrow().intl_segmenter.as_deref().cloned())
        .ok_or_else(brand_error)
}
fn segments(this: &Value) -> Result<Rc<SegmentsSlots>, VmErr> {
    this.property_cell()
        .and_then(|p| p.meta.borrow().intl_segments.clone())
        .ok_or_else(brand_error)
}
fn method(target: &Value, name: &str, length: usize, f: NativeFn, fp: &Option<Value>) {
    nonenum(target, name, native_method(name, length, f, fp.clone()));
}
fn iterator_method(target: &Value, f: NativeFn, fp: &Option<Value>) {
    if let Some(Value::Symbol(ref symbol)) = super::super::well_known("iterator") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        nonenum(
            target,
            &key,
            native_method("[Symbol.iterator]", 0, f, fp.clone()),
        );
        target
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_symbol_key(&key, symbol.clone());
    }
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|o| o.get_prop("prototype"));
    let constructor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&constructor, super::super::require_new, Some(construct));
    for (key, value) in [
        ("name", Value::String("Segmenter".into())),
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
    let prototype = Value::object_with_proto(vec![], op.clone().map(Rc::new));
    nonenum(&prototype, "constructor", constructor.clone());
    tag(&prototype, "Intl.Segmenter");
    method(&prototype, "segment", 1, segment, &fp);
    method(&prototype, "resolvedOptions", 0, resolved_options, &fp);
    super::super::set_builtin_constructor_prototype(e, &constructor, prototype.clone());
    nonenum(namespace, "Segmenter", constructor.clone());
    e.install_intrinsic("Intl.Segmenter", constructor);
    e.install_intrinsic("Intl.Segmenter.prototype", prototype);
    let prototype = Value::object_with_proto(vec![], op.map(Rc::new));
    method(&prototype, "containing", 1, containing, &fp);
    iterator_method(&prototype, iterator, &fp);
    e.install_intrinsic("%SegmentsPrototype%", prototype);
    let parent = e.intrinsic("%IteratorPrototype%").unwrap();
    let prototype = Value::object_with_proto(vec![], Some(Rc::new(parent)));
    method(&prototype, "next", 0, next, &fp);
    tag(&prototype, "Segmenter String Iterator");
    e.install_intrinsic("%SegmentIteratorPrototype%", prototype);
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let target = i.new_target_stack.last().cloned().ok_or_else(brand_error)?;
    let prototype = i.constructor_prototype(&target, "Intl.Segmenter")?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        value if crate::interpreter::call::is_js_object(&value) => value,
        _ => {
            return Err(VmErr::Msg(
                "TypeError: Segmenter options must be an object".into(),
            ));
        }
    };
    locale_matcher(i, &options)?;
    let locale = requested
        .iter()
        .find_map(|tag| best_available_locale(tag))
        .unwrap_or_else(|| "en-US".into());
    let granularity = match option(i, &options, "granularity")?.as_deref() {
        None | Some("grapheme") => Granularity::Grapheme,
        Some("word") => Granularity::Word,
        Some("sentence") => Granularity::Sentence,
        _ => {
            return Err(VmErr::Msg(
                "RangeError: Invalid segmentation granularity".into(),
            ));
        }
    };
    let result = Value::object_with_proto(vec![], prototype);
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_segmenter = Some(Box::new(SegmenterSlots {
        locale,
        granularity,
    }));
    Ok(result)
}
fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let slots = segmenter(&this)?;
    Ok(Value::object(vec![
        ("locale".into(), Value::String(slots.locale.into())),
        (
            "granularity".into(),
            Value::String(slots.granularity.name().into()),
        ),
    ]))
}
fn segment(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let slots = segmenter(&this)?;
    let input = i.ecmascript_to_string(&arg(&args, 0))?;
    let mut boundaries = Vec::new();
    match slots.granularity {
        Granularity::Grapheme => {
            let segmenter = GraphemeClusterSegmenter::new();
            let mut breaks = segmenter.segment_utf16(input.units());
            let mut start = breaks.next().unwrap_or(0);
            for end in breaks {
                i.consume_loop()?;
                boundaries.push(Boundary {
                    start,
                    end,
                    word_like: false,
                });
                start = end;
            }
        }
        Granularity::Word => {
            let segmenter = WordSegmenter::new_auto(Default::default());
            let mut breaks = segmenter.segment_utf16(input.units());
            let mut start = breaks.next().unwrap_or(0);
            while let Some(end) = breaks.next() {
                i.consume_loop()?;
                boundaries.push(Boundary {
                    start,
                    end,
                    word_like: breaks.word_type().is_word_like(),
                });
                start = end;
            }
        }
        Granularity::Sentence => {
            let segmenter = SentenceSegmenter::new(Default::default());
            let mut breaks = segmenter.segment_utf16(input.units());
            let mut start = breaks.next().unwrap_or(0);
            for end in breaks {
                i.consume_loop()?;
                boundaries.push(Boundary {
                    start,
                    end,
                    word_like: false,
                });
                start = end;
            }
        }
    }
    let prototype = i
        .persistent_global
        .borrow()
        .intrinsic("%SegmentsPrototype%")
        .unwrap();
    let result = Value::object_with_proto(vec![], Some(Rc::new(prototype)));
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_segments = Some(Rc::new(SegmentsSlots {
        input,
        boundaries,
        word: slots.granularity == Granularity::Word,
    }));
    Ok(result)
}
fn record(data: &SegmentsSlots, index: usize) -> Value {
    let boundary = &data.boundaries[index];
    let mut fields = vec![
        (
            "segment".into(),
            Value::String(data.input.slice(boundary.start, boundary.end)),
        ),
        ("index".into(), Value::Number(boundary.start as f64)),
        ("input".into(), Value::String(data.input.clone())),
    ];
    if data.word {
        fields.push(("isWordLike".into(), Value::Bool(boundary.word_like)));
    }
    Value::object(fields)
}
fn containing(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let data = segments(&this)?;
    let index = crate::value::to_integer_or_infinity(i.ecmascript_to_number(&arg(&args, 0))?);
    if index < 0. || index >= data.input.len() as f64 {
        return Ok(Value::Undefined);
    }
    let index = index as usize;
    let found = data
        .boundaries
        .partition_point(|boundary| boundary.end <= index);
    Ok(data
        .boundaries
        .get(found)
        .map(|_| record(&data, found))
        .unwrap_or(Value::Undefined))
}
fn iterator(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let segments = segments(&this)?;
    let prototype = i
        .persistent_global
        .borrow()
        .intrinsic("%SegmentIteratorPrototype%")
        .unwrap();
    let result = Value::object_with_proto(vec![], Some(Rc::new(prototype)));
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_segment_iterator = Some(Rc::new(IteratorSlots {
        segments,
        cursor: Cell::new(0),
    }));
    Ok(result)
}
fn next(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let slots = this
        .property_cell()
        .and_then(|p| p.meta.borrow().intl_segment_iterator.clone())
        .ok_or_else(brand_error)?;
    let index = slots.cursor.get();
    if index >= slots.segments.boundaries.len() {
        return Ok(crate::interpreter::call::iter_result(
            Value::Undefined,
            true,
        ));
    }
    slots.cursor.set(index + 1);
    Ok(crate::interpreter::call::iter_result(
        record(&slots.segments, index),
        false,
    ))
}
