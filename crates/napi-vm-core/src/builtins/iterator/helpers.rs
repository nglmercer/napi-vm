//! Synchronous Iterator helpers, with traced internal records and shared closing semantics.
use crate::interpreter::call::{is_js_object, iter_result};
use crate::{
    error::VmErr,
    interpreter::{Environment, Interpreter, is_callable_value, strict_equals},
    value::{PropAttrs, Value},
};
use std::{cell::RefCell, rc::Rc};

fn error(message: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {message}"))
}
fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Undefined)
}
#[derive(Clone)]
struct Record {
    object: Value,
    next: Value,
    done: bool,
}
impl Record {
    fn direct(vm: &mut Interpreter, object: Value) -> Result<Self, VmErr> {
        if !is_js_object(&object) {
            return Err(error("Iterator must be an object"));
        }
        let next = vm.get_prop_value_str(&object, "next")?;
        Ok(Self {
            object,
            next,
            done: false,
        })
    }
    fn step(&mut self, vm: &mut Interpreter, value: bool) -> Result<Option<Value>, VmErr> {
        if self.done {
            return Ok(None);
        }
        let result = (|| {
            vm.consume_fuel(1)?;
            let result = vm.call_this(&self.next, self.object.clone(), vec![])?;
            if !is_js_object(&result) {
                return Err(error("Iterator result must be an object"));
            }
            if vm.get_prop_value_str(&result, "done")?.is_truthy() {
                return Ok(None);
            }
            Ok(Some(if value {
                vm.get_prop_value_str(&result, "value")?
            } else {
                Value::Undefined
            }))
        })();
        if !matches!(result, Ok(Some(_))) {
            self.done = true;
        }
        result
    }
    fn close(&mut self, vm: &mut Interpreter) -> Result<(), VmErr> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        vm.close_guest_iterator(&self.object, false)
    }
    fn abrupt<T>(&mut self, vm: &mut Interpreter, err: VmErr) -> Result<T, VmErr> {
        let _ = self.close(vm);
        Err(err)
    }
}
fn flatten(vm: &mut Interpreter, source: Value, strings: bool) -> Result<Record, VmErr> {
    if !is_js_object(&source) && !(strings && matches!(source, Value::String(_))) {
        return Err(error("Expected an iterable object"));
    }
    let key = super::super::well_known("iterator").unwrap();
    let object = match vm.get_method(&source, &key)? {
        Some(method) => vm.iterator_from_method(&source, &method)?,
        None => source,
    };
    Record::direct(vm, object)
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Wrap,
    Map,
    Filter,
    FlatMap,
    Take,
    Drop,
    Chunks,
    Windows,
    Concat,
    Zip,
}
#[derive(Clone)]
pub(crate) struct Slots {
    kind: Kind,
    executing: bool,
    complete: bool,
    started: bool,
    records: Vec<Record>,
    inner: Option<Record>,
    callback: Value,
    counter: f64,
    remaining: f64,
    buffer: Vec<Value>,
    sources: Vec<(Value, Value)>,
    index: usize,
    partial: bool,
    mode: String,
    padding: Vec<Value>,
    keys: Option<Vec<Value>>,
}
impl Slots {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            executing: false,
            complete: false,
            started: false,
            records: vec![],
            inner: None,
            callback: Value::Undefined,
            counter: 0.,
            remaining: 0.,
            buffer: vec![],
            sources: vec![],
            index: 0,
            partial: false,
            mode: "shortest".into(),
            padding: vec![],
            keys: None,
        }
    }
    pub(crate) fn values(&self) -> Vec<Value> {
        let mut values = vec![self.callback.clone()];
        for record in self.records.iter().chain(self.inner.iter()) {
            values.extend([record.object.clone(), record.next.clone()]);
        }
        values.extend(self.buffer.clone());
        values.extend(self.padding.clone());
        for (object, method) in &self.sources {
            values.extend([object.clone(), method.clone()]);
        }
        if let Some(keys) = &self.keys {
            values.extend(keys.clone());
        }
        values
    }
    fn close(&mut self, vm: &mut Interpreter) -> Result<(), VmErr> {
        let mut result = Ok(());
        if let Some(inner) = &mut self.inner {
            result = inner.close(vm);
        }
        for record in self.records.iter_mut().rev() {
            let closed = record.close(vm);
            if result.is_ok() {
                result = closed;
            }
        }
        result
    }
    fn advance(&mut self, vm: &mut Interpreter) -> Result<Option<Value>, VmErr> {
        if self.kind == Kind::Concat {
            loop {
                if let Some(inner) = &mut self.inner {
                    if let Some(value) = inner.step(vm, true)? {
                        return Ok(Some(value));
                    }
                    self.inner = None;
                }
                let Some((source, method)) = self.sources.get(self.index).cloned() else {
                    return Ok(None);
                };
                self.index += 1;
                let object = vm.iterator_from_method(&source, &method)?;
                self.inner = Some(Record::direct(vm, object)?);
            }
        }
        if self.kind == Kind::Zip {
            return self.zip_next(vm);
        }
        if self.kind == Kind::Take && self.remaining == 0. {
            self.records[0].close(vm)?;
            return Ok(None);
        }
        if self.kind == Kind::Drop {
            while self.remaining > 0. {
                self.remaining -= 1.;
                if self.records[0].step(vm, false)?.is_none() {
                    return Ok(None);
                }
            }
        }
        loop {
            if self.kind == Kind::FlatMap
                && let Some(inner) = &mut self.inner
            {
                match inner.step(vm, true) {
                    Ok(Some(value)) => return Ok(Some(value)),
                    Ok(None) => self.inner = None,
                    Err(err) => return self.records[0].abrupt(vm, err),
                }
            }
            if self.kind == Kind::Take {
                self.remaining -= 1.;
            }
            let Some(value) = self.records[0].step(vm, true)? else {
                if !self.buffer.is_empty()
                    && (self.kind == Kind::Chunks
                        || (self.kind == Kind::Windows
                            && self.partial
                            && self.buffer.len() < (self.remaining as usize)))
                {
                    return Ok(Some(Value::checked_array(std::mem::take(
                        &mut self.buffer,
                    ))?));
                }
                return Ok(None);
            };
            match self.kind {
                Kind::Map | Kind::Filter | Kind::FlatMap => {
                    let mapped = match vm.call_this(
                        &self.callback,
                        Value::Undefined,
                        vec![value.clone(), Value::Number(self.counter)],
                    ) {
                        Ok(v) => v,
                        Err(err) => return self.records[0].abrupt(vm, err),
                    };
                    self.counter += 1.;
                    if self.kind == Kind::Map {
                        return Ok(Some(mapped));
                    }
                    if self.kind == Kind::Filter {
                        if mapped.is_truthy() {
                            return Ok(Some(value));
                        }
                    } else {
                        self.inner = Some(match flatten(vm, mapped, false) {
                            Ok(record) => record,
                            Err(err) => return self.records[0].abrupt(vm, err),
                        });
                    }
                }
                Kind::Chunks | Kind::Windows => {
                    if self.kind == Kind::Windows && self.buffer.len() == self.remaining as usize {
                        self.buffer.remove(0);
                    }
                    if self.buffer.len() >= crate::value::MAX_ARRAY_LEN {
                        return self.records[0].abrupt(
                            vm,
                            VmErr::Msg("RangeError: Maximum array length exceeded".into()),
                        );
                    }
                    self.buffer.push(value);
                    if self.buffer.len() == self.remaining as usize {
                        let values = if self.kind == Kind::Chunks {
                            std::mem::take(&mut self.buffer)
                        } else {
                            self.buffer.clone()
                        };
                        return Ok(Some(Value::checked_array(values)?));
                    }
                }
                _ => return Ok(Some(value)),
            }
        }
    }
    fn zip_next(&mut self, vm: &mut Interpreter) -> Result<Option<Value>, VmErr> {
        if self.records.is_empty() {
            return Ok(None);
        }
        let mut row = Vec::new();
        for i in 0..self.records.len() {
            let step = match self.records[i].step(vm, true) {
                Ok(value) => value,
                Err(err) => {
                    let _ = self.close(vm);
                    return Err(err);
                }
            };
            match step {
                Some(value) => row.push(value),
                None if self.mode == "longest" => row.push(self.padding[i].clone()),
                None if self.mode == "strict" => {
                    let mut mismatch = i != 0;
                    if !mismatch {
                        for j in i + 1..self.records.len() {
                            match self.records[j].step(vm, false) {
                                Ok(value) => {
                                    if value.is_some() {
                                        mismatch = true;
                                        break;
                                    }
                                }
                                Err(err) => {
                                    let _ = self.close(vm);
                                    return Err(err);
                                }
                            }
                        }
                    }
                    if mismatch {
                        let _ = self.close(vm);
                        return Err(error("Iterator lengths differ"));
                    }
                    return Ok(None);
                }
                None => {
                    self.close(vm)?;
                    return Ok(None);
                }
            }
        }
        if self.records.iter().all(|record| record.done) {
            return Ok(None);
        }
        if let Some(keys) = &self.keys {
            let result = Value::object_with_proto(vec![], None);
            for (key, value) in keys.iter().zip(row) {
                let descriptor = Value::object(vec![
                    ("value".into(), value),
                    ("writable".into(), Value::Bool(true)),
                    ("enumerable".into(), Value::Bool(true)),
                    ("configurable".into(), Value::Bool(true)),
                ]);
                vm.define_own_property(&result, key, &descriptor)?;
            }
            Ok(Some(result))
        } else {
            Ok(Some(Value::checked_array(row)?))
        }
    }
}
fn make(vm: &mut Interpreter, slots: Slots) -> Value {
    let name = if slots.kind == Kind::Wrap {
        "%WrapForValidIteratorPrototype%"
    } else {
        "%IteratorHelperPrototype%"
    };
    let proto = vm.persistent_global.borrow().intrinsic(name);
    let value = Value::object_with_proto(vec![], proto.map(Rc::new));
    value
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .iterator_helper = Some(Rc::new(RefCell::new(slots)));
    value
}
fn slots(this: &Value) -> Result<Rc<RefCell<Slots>>, VmErr> {
    if let Value::Object { props: object } = this
        && let Some(slots) = &object.meta.borrow().iterator_helper
    {
        return Ok(slots.clone());
    }
    Err(error("Incompatible iterator helper receiver"))
}
fn wrap_next(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if slots(&this)?.borrow().kind != Kind::Wrap {
        return Err(error("Incompatible iterator wrapper receiver"));
    }
    perform_next(vm, this, args)
}
fn next(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if slots(&this)?.borrow().kind == Kind::Wrap {
        return Err(error("Incompatible iterator helper receiver"));
    }
    perform_next(vm, this, args)
}
fn perform_next(vm: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let cell = slots(&this)?;
    let mut state = cell.borrow().clone();
    if state.kind == Kind::Wrap {
        return vm.call_this(
            &state.records[0].next,
            state.records[0].object.clone(),
            vec![],
        );
    }
    if state.executing {
        return Err(error("Iterator helper is already executing"));
    }
    if state.complete {
        return Ok(iter_result(Value::Undefined, true));
    }
    cell.borrow_mut().executing = true;
    state.started = true;
    let result = state.advance(vm);
    state.complete = !matches!(result, Ok(Some(_)));
    state.executing = false;
    *cell.borrow_mut() = state;
    result.map(|value| match value {
        Some(value) => iter_result(value, false),
        None => iter_result(Value::Undefined, true),
    })
}
fn wrap_return(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if slots(&this)?.borrow().kind != Kind::Wrap {
        return Err(error("Incompatible iterator wrapper receiver"));
    }
    perform_return(vm, this, args)
}
fn return_(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if slots(&this)?.borrow().kind == Kind::Wrap {
        return Err(error("Incompatible iterator helper receiver"));
    }
    perform_return(vm, this, args)
}
fn perform_return(vm: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let cell = slots(&this)?;
    let mut state = cell.borrow().clone();
    if state.kind == Kind::Wrap {
        let object = &state.records[0].object;
        return match vm.get_method(object, &Value::String("return".into()))? {
            Some(method) => vm.call_this(&method, object.clone(), vec![]),
            None => Ok(iter_result(Value::Undefined, true)),
        };
    }
    if state.executing {
        return Err(error("Iterator helper is already executing"));
    }
    if state.complete {
        return Ok(iter_result(Value::Undefined, true));
    }
    {
        let mut stored = cell.borrow_mut();
        stored.complete = !state.started;
        stored.executing = state.started;
    }
    let result = state.close(vm);
    state.complete = true;
    state.executing = false;
    *cell.borrow_mut() = state;
    result.map(|_| iter_result(Value::Undefined, true))
}
fn close_error<T>(vm: &mut Interpreter, this: &Value, err: VmErr) -> Result<T, VmErr> {
    let _ = vm.close_guest_iterator(this, false);
    Err(err)
}
fn lazy(vm: &mut Interpreter, this: Value, args: Vec<Value>, kind: Kind) -> Result<Value, VmErr> {
    if !is_js_object(&this) {
        return Err(error("Iterator receiver must be an object"));
    }
    let mut state = Slots::new(kind);
    let parameter = arg(&args, 0);
    if matches!(kind, Kind::Map | Kind::Filter | Kind::FlatMap) {
        if !is_callable_value(&parameter) {
            return close_error(vm, &this, error("Callback must be callable"));
        }
        state.callback = parameter;
    } else {
        let size = if matches!(kind, Kind::Chunks | Kind::Windows) {
            match parameter {
                Value::Number(n) if n.is_finite() && n.fract() == 0. => n,
                _ => return close_error(vm, &this, error("Size must be an integer number")),
            }
        } else {
            match vm.ecmascript_to_number(&parameter) {
                Ok(n) => n,
                Err(err) => return close_error(vm, &this, err),
            }
        };
        let size = if size.is_nan() {
            return close_error(
                vm,
                &this,
                VmErr::Msg("RangeError: Invalid iterator limit".into()),
            );
        } else {
            size.trunc()
        };
        let invalid = if matches!(kind, Kind::Chunks | Kind::Windows) {
            size < 1. || size > u32::MAX as f64
        } else {
            size < 0. || (size.is_finite() && size > 9007199254740991.)
        };
        if invalid {
            return close_error(
                vm,
                &this,
                VmErr::Msg("RangeError: Invalid iterator size".into()),
            );
        }
        state.remaining = size;
        if kind == Kind::Windows {
            state.partial = match arg(&args, 1) {
                Value::Undefined => false,
                Value::String(ref s) if s == "only-full" => false,
                Value::String(ref s) if s == "allow-partial" => true,
                _ => return close_error(vm, &this, error("Invalid undersized mode")),
            };
        }
    }
    state.records.push(Record::direct(vm, this)?);
    Ok(make(vm, state))
}
macro_rules! lazy_methods {($($name:ident:$kind:ident),*)=>{$(fn $name(vm:&mut Interpreter,this:Value,args:Vec<Value>)->Result<Value,VmErr>{lazy(vm,this,args,Kind::$kind)})*};}
lazy_methods!(map:Map,filter:Filter,flat_map:FlatMap,take:Take,drop_:Drop,chunks:Chunks,windows:Windows);
fn from(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let record = flatten(vm, arg(&args, 0), true)?;
    // The intrinsic prototype identity is independent of a modified Iterator constructor.
    let prototype = vm
        .persistent_global
        .borrow()
        .intrinsic("%IteratorPrototype%")
        .unwrap();
    let mut current = record.object.clone();
    for _ in 0..1024 {
        if strict_equals(&current, &prototype) {
            return Ok(record.object);
        }
        let parent = vm.get_prototype_of(&current)?;
        if matches!(parent, Value::Null) {
            break;
        }
        current = parent;
    }
    let mut state = Slots::new(Kind::Wrap);
    state.records.push(record);
    Ok(make(vm, state))
}
fn concat(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let mut state = Slots::new(Kind::Concat);
    for source in args {
        if !is_js_object(&source) {
            return Err(error("concat requires iterable objects"));
        }
        let method = vm
            .get_method(&source, &super::super::well_known("iterator").unwrap())?
            .ok_or_else(|| error("Value is not iterable"))?;
        state.sources.push((source, method));
    }
    Ok(make(vm, state))
}
fn zip(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    zip_create(vm, args, false)
}
fn zip_keyed(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    zip_create(vm, args, true)
}
fn zip_create(vm: &mut Interpreter, args: Vec<Value>, keyed: bool) -> Result<Value, VmErr> {
    let source = arg(&args, 0);
    if !is_js_object(&source) {
        return Err(error("zip requires an object"));
    }
    let options = arg(&args, 1);
    if !matches!(options, Value::Undefined) && !is_js_object(&options) {
        return Err(error("Options must be an object"));
    }
    let mut state = Slots::new(Kind::Zip);
    if !matches!(options, Value::Undefined) {
        state.mode = match vm.get_prop_value_str(&options, "mode")? {
            Value::Undefined => "shortest".into(),
            Value::String(ref s) if s == "shortest" || s == "longest" || s == "strict" => {
                s.to_string()
            }
            _ => return Err(error("Invalid zip mode")),
        };
    }
    let padding = if state.mode == "longest" && !matches!(options, Value::Undefined) {
        vm.get_prop_value_str(&options, "padding")?
    } else {
        Value::Undefined
    };
    if !matches!(padding, Value::Undefined) && !is_js_object(&padding) {
        return Err(error("Padding must be an object"));
    }
    let collect = (|| {
        if keyed {
            let mut keys = vec![];
            for key in vm.own_property_keys(&source)? {
                let descriptor = crate::builtins::object::descriptor_for_key_in(vm, &source, &key)?;
                if matches!(descriptor, Value::Undefined)
                    || !vm
                        .get_prop_value_str(&descriptor, "enumerable")?
                        .is_truthy()
                {
                    continue;
                }
                let value = vm.get_prop_value(&source, &key)?;
                if matches!(value, Value::Undefined) {
                    continue;
                }
                state.records.push(flatten(vm, value, false)?);
                keys.push(key);
            }
            state.keys = Some(keys);
        } else {
            let object = vm.iterator_for(&source)?;
            let mut outer = Record::direct(vm, object)?;
            loop {
                let value = outer.step(vm, true)?;
                let Some(value) = value else { break };
                match flatten(vm, value, false) {
                    Ok(record) => state.records.push(record),
                    Err(err) => {
                        let _ = state.close(vm);
                        return outer.abrupt(vm, err);
                    }
                }
            }
        }
        state.padding = vec![Value::Undefined; state.records.len()];
        if !matches!(padding, Value::Undefined) {
            if let Some(keys) = &state.keys {
                for (i, key) in keys.iter().enumerate() {
                    state.padding[i] = vm.get_prop_value(&padding, key)?;
                }
            } else {
                let object = vm.iterator_for(&padding)?;
                let mut record = Record::direct(vm, object)?;
                for value in &mut state.padding {
                    if let Some(item) = record.step(vm, true)? {
                        *value = item;
                    }
                }
                record.close(vm)?;
            }
        }
        Ok(())
    })();
    if let Err(err) = collect {
        let _ = state.close(vm);
        return Err(err);
    }
    Ok(make(vm, state))
}
fn eager(
    vm: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    operation: &str,
) -> Result<Value, VmErr> {
    if !is_js_object(&this) {
        return Err(error("Iterator receiver must be an object"));
    }
    let callback = arg(&args, 0);
    if matches!(operation, "reduce" | "forEach" | "some" | "every" | "find")
        && !is_callable_value(&callback)
    {
        return close_error(vm, &this, error("Callback must be callable"));
    }
    let mut skip = 0.;
    if operation == "includes" && !matches!(arg(&args, 1), Value::Undefined) {
        skip = match arg(&args, 1) {
            Value::Number(n) if !n.is_nan() && n.trunc() == n => n,
            _ => return close_error(vm, &this, error("fromIndex must be an integer number")),
        };
        if skip < 0. || (skip.is_finite() && skip > 9007199254740991.) {
            return close_error(
                vm,
                &this,
                VmErr::Msg("RangeError: Invalid fromIndex".into()),
            );
        }
    }
    let separator = if operation == "join" {
        if matches!(callback, Value::Undefined) {
            ",".into()
        } else {
            match vm.ecmascript_to_string(&callback) {
                Ok(s) => s,
                Err(err) => return close_error(vm, &this, err),
            }
        }
    } else {
        "".into()
    };
    let mut record = Record::direct(vm, this)?;
    while skip > 0. {
        skip -= 1.;
        if record.step(vm, false)?.is_none() {
            return Ok(Value::Bool(false));
        }
    }
    let mut values = vec![];
    let mut index = 0.;
    let mut accumulator = arg(&args, 1);
    let mut text = crate::JsString::from("");
    if operation == "reduce" && args.len() < 2 {
        accumulator = record
            .step(vm, true)?
            .ok_or_else(|| error("Reduce of empty iterator"))?;
        index = 1.;
    }
    while let Some(value) = record.step(vm, true)? {
        let result = (|| match operation {
            "toArray" => {
                if values.len() >= crate::value::MAX_ARRAY_LEN {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum array length exceeded".into(),
                    ));
                }
                values.push(value);
                Ok(None)
            }
            "join" => {
                if index > 0. {
                    if text.len().saturating_add(separator.len()) > crate::value::MAX_STRING_LEN {
                        return Err(VmErr::Msg(
                            "RangeError: Maximum string length exceeded".into(),
                        ));
                    }
                    text = text.concat(&separator);
                }
                if !matches!(value, Value::Null | Value::Undefined) {
                    let part = vm.ecmascript_to_string(&value)?;
                    if text.len().saturating_add(part.len()) > crate::value::MAX_STRING_LEN {
                        return Err(VmErr::Msg(
                            "RangeError: Maximum string length exceeded".into(),
                        ));
                    }
                    text = text.concat(&part);
                }
                Ok(None)
            }
            "includes" => {
                if strict_equals(&value, &callback)
                    || matches!((&value,&callback),(Value::Number(a),Value::Number(b)) if a.is_nan()&&b.is_nan())
                {
                    Ok(Some(Value::Bool(true)))
                } else {
                    Ok(None)
                }
            }
            "reduce" => {
                accumulator = vm.call_this(
                    &callback,
                    Value::Undefined,
                    vec![accumulator.clone(), value, Value::Number(index)],
                )?;
                Ok(None)
            }
            _ => {
                let mapped = vm.call_this(
                    &callback,
                    Value::Undefined,
                    vec![value.clone(), Value::Number(index)],
                )?;
                Ok(match operation {
                    "some" if mapped.is_truthy() => Some(Value::Bool(true)),
                    "every" if !mapped.is_truthy() => Some(Value::Bool(false)),
                    "find" if mapped.is_truthy() => Some(value),
                    _ => None,
                })
            }
        })();
        match result {
            Ok(Some(value)) => {
                record.close(vm)?;
                return Ok(value);
            }
            Ok(None) => (),
            Err(err) => return record.abrupt(vm, err),
        }
        index += 1.;
    }
    match operation {
        "toArray" => Value::checked_array(values),
        "join" => Ok(Value::String(text)),
        "reduce" => Ok(accumulator),
        "some" | "includes" => Ok(Value::Bool(false)),
        "every" => Ok(Value::Bool(true)),
        _ => Ok(Value::Undefined),
    }
}
macro_rules! eager_methods {($($name:ident:$op:literal),*)=>{$(fn $name(vm:&mut Interpreter,this:Value,args:Vec<Value>)->Result<Value,VmErr>{eager(vm,this,args,$op)})*};}
eager_methods!(to_array:"toArray",reduce:"reduce",for_each:"forEach",some:"some",every:"every",find:"find",includes:"includes",join:"join");
fn dispose(vm: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(method) = vm.get_method(&this, &Value::String("return".into()))? {
        vm.call_this(&method, this, vec![])?;
    }
    Ok(Value::Undefined)
}
fn method(
    env: &Environment,
    object: &Value,
    name: &str,
    length: usize,
    callback: crate::builtins::NativeFn,
) {
    let function = crate::builtins::native_method(
        name,
        length,
        callback,
        env.get("Function")
            .and_then(|value| value.get_prop("prototype")),
    );
    object.set_prop(name.into(), function).unwrap();
    object.property_cell().unwrap().meta.borrow_mut().set_attrs(
        name,
        PropAttrs {
            writable: true,
            enumerable: false,
            configurable: true,
        },
    );
}
pub(super) fn install(env: &mut Environment, constructor: &Value, prototype: &Value) {
    for (name, length, callback) in [
        ("from", 1, from as crate::builtins::NativeFn),
        ("concat", 0, concat),
        ("zip", 1, zip),
        ("zipKeyed", 1, zip_keyed),
    ] {
        method(env, constructor, name, length, callback);
    }
    for (name, length, callback) in [
        ("map", 1, map as crate::builtins::NativeFn),
        ("filter", 1, filter),
        ("flatMap", 1, flat_map),
        ("take", 1, take),
        ("drop", 1, drop_),
        ("chunks", 1, chunks),
        ("windows", 1, windows),
        ("toArray", 0, to_array),
        ("reduce", 1, reduce),
        ("forEach", 1, for_each),
        ("some", 1, some),
        ("every", 1, every),
        ("find", 1, find),
        ("includes", 1, includes),
        ("join", 1, join),
    ] {
        method(env, prototype, name, length, callback);
    }
    let Value::Symbol(ref symbol) = crate::builtins::well_known("dispose").unwrap() else {
        unreachable!()
    };
    let key = crate::interpreter::symbol_slot_key(symbol);
    method(env, prototype, &key, 0, dispose);
    let dispose_method = crate::builtins::native_method(
        "[Symbol.dispose]",
        0,
        dispose,
        env.get("Function")
            .and_then(|value| value.get_prop("prototype")),
    );
    prototype.set_prop(key.clone(), dispose_method).unwrap();
    prototype
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .set_symbol_key(&key, symbol.clone());
    for (name, tag) in [
        ("%IteratorHelperPrototype%", true),
        ("%WrapForValidIteratorPrototype%", false),
    ] {
        let object = Value::object_with_proto(vec![], Some(Rc::new(prototype.clone())));
        method(env, &object, "next", 0, if tag { next } else { wrap_next });
        method(
            env,
            &object,
            "return",
            0,
            if tag { return_ } else { wrap_return },
        );
        if tag {
            let Value::Symbol(ref symbol) = crate::builtins::well_known("toStringTag").unwrap()
            else {
                unreachable!()
            };
            let key = crate::interpreter::symbol_slot_key(symbol);
            object
                .set_prop(key.clone(), Value::String("Iterator Helper".into()))
                .unwrap();
            let cell = object.property_cell().unwrap();
            let mut meta = cell.meta.borrow_mut();
            meta.set_symbol_key(&key, symbol.clone());
            meta.set_attrs(
                &key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        env.install_intrinsic(name, object);
    }
}
