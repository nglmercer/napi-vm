//! Function and constructor calls, destructuring binding, catch handling,
//! and member assignment.

use std::cell::RefCell;
use std::rc::Rc;

use smallvec::SmallVec;

use super::{Environment, Interpreter};
use crate::error::{RuntimeErrorData, VmErr, vm_err};
use crate::parser::{Pattern, Statement};
use crate::span::Span;
#[cfg(stackful_coroutines)]
use crate::value::{GenOutcome, GenResume};
use crate::value::{GeneratorInner, ObjectCell, PromiseState, Value};

type Key = Rc<str>;

/// Internal slot naming the function a built-in namespace object runs when it
/// is *called*: `String(x)`, `Number(x)`, `Map(…)`.
pub(crate) const CALL_SLOT: &str = "__symbol_call__";
/// Internal slot naming the function it runs when it is *constructed*, for
/// built-ins whose `new` form differs from their call form.
pub(crate) const CONSTRUCT_SLOT: &str = "__symbol_construct__";

/// The function stored in one of the internal call slots, if any.
pub(crate) fn callable_slot(value: &Value, slot: &str) -> Option<Value> {
    let Value::Object { props } = value else {
        return None;
    };
    props
        .borrow()
        .iter()
        .find(|(k, _)| k == slot)
        .map(|(_, v)| v.clone())
        .filter(|v| {
            matches!(
                v,
                Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
            )
        })
}

pub(crate) fn is_callable_value(value: &Value) -> bool {
    match value {
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => true,
        Value::Proxy(proxy) => is_callable_value(&proxy.target),
        Value::Object { .. } => callable_slot(value, CALL_SLOT).is_some(),
        _ => false,
    }
}

fn is_setter_value(value: &Value) -> bool {
    match value {
        Value::Function(function) => function
            .name
            .as_ref()
            .is_some_and(|name| name.starts_with("set ")),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            name.starts_with("set ")
        }
        _ => false,
    }
}

fn is_getter_value(value: &Value) -> bool {
    match value {
        Value::Function(function) => function
            .name
            .as_ref()
            .is_some_and(|name| name.starts_with("get ")),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            name.starts_with("get ")
        }
        _ => false,
    }
}

fn array_property_setter(
    array: &crate::value::ArrayCell,
    key: &str,
    current: Option<&Value>,
) -> Option<Value> {
    if let Some(current) = current.filter(|value| is_setter_value(value)) {
        return Some(current.clone());
    }
    if current.is_some_and(is_getter_value) {
        return array
            .named_prop(&format!("__setter:{}__", key))
            .filter(is_setter_value);
    }
    None
}

fn is_js_object(value: &Value) -> bool {
    if matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::BigInt(_)
            | Value::Symbol(_)
            | Value::HostPending { .. }
            | Value::Binding(_)
    ) {
        return false;
    }
    #[cfg(stackful_coroutines)]
    if matches!(value, Value::AsyncTask(_)) {
        return false;
    }
    true
}

impl Interpreter {
    /// ECMAScript `instanceof`, including a guest-defined
    /// `Symbol.hasInstance` method. This lives on the mutable interpreter
    /// path because reading the method and invoking it can execute guest code.
    pub(crate) fn instance_of(
        &mut self,
        object: &Value,
        constructor: &Value,
    ) -> Result<Value, VmErr> {
        let symbol = crate::builtins::well_known("hasInstance")
            .expect("Symbol.hasInstance is a well-known symbol");
        let method = self.get_prop_value(constructor, &symbol)?;
        if !matches!(method, Value::Undefined | Value::Null) {
            if !is_callable_value(&method) {
                return Err(VmErr::Msg(
                    "TypeError: Symbol.hasInstance is not callable".into(),
                ));
            }
            let result = self.call_this(&method, constructor.clone(), vec![object.clone()])?;
            return Ok(Value::Bool(self.truthy(&result)));
        }
        if !is_callable_value(constructor) {
            return Err(VmErr::Msg(
                "TypeError: Right-hand side of instanceof is not callable".into(),
            ));
        }

        self.ordinary_instance_of(object, constructor)
    }

    /// Perform the ordinary prototype-based `instanceof` check without
    /// looking up `Symbol.hasInstance`. The intrinsic method uses this path
    /// after the operator has selected it.
    pub(crate) fn ordinary_instance_of(
        &mut self,
        object: &Value,
        constructor: &Value,
    ) -> Result<Value, VmErr> {
        if !is_callable_value(constructor) {
            return Ok(Value::Bool(false));
        }
        if let Value::Function(function) = constructor
            && let Some(bound) = &function.bound
        {
            return self.instance_of(object, &bound.target);
        }

        if matches!(object, Value::Date(_))
            && matches!(constructor, Value::Object { props }
                if props.meta.borrow().builtin_constructor == Some(crate::value::BuiltinConstructor::Date))
        {
            return Ok(Value::Bool(true));
        }
        if let (Value::Error(error), Value::Class(class)) = (object, constructor) {
            return Ok(Value::Bool(
                class.name == "Error" || class.name == error.name,
            ));
        }

        let prototype =
            self.get_prop_value(constructor, &Value::String("prototype".to_string()))?;
        if !is_js_object(&prototype) {
            return Err(VmErr::Msg(
                "TypeError: Function has non-object prototype in instanceof check".into(),
            ));
        }
        if !is_js_object(object) {
            return Ok(Value::Bool(false));
        }

        let mut current = self.get_prototype_of(object)?;
        let mut visited = std::collections::HashSet::new();
        for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
            if matches!(current, Value::Null) {
                return Ok(Value::Bool(false));
            }
            if crate::interpreter::strict_equals(&current, &prototype) {
                return Ok(Value::Bool(true));
            }
            let identity = match &current {
                Value::Object { props } => Rc::as_ptr(props) as usize,
                Value::Array(array) => Rc::as_ptr(array) as usize,
                Value::Class(class) => Rc::as_ptr(&class.statics) as usize,
                Value::Function(function) => Rc::as_ptr(&function.properties) as usize,
                Value::Proxy(proxy) => Rc::as_ptr(proxy) as usize,
                _ => return Ok(Value::Bool(false)),
            };
            if !visited.insert(identity) {
                return Err(crate::value::limit_err(
                    "Maximum prototype chain depth exceeded",
                ));
            }
            current = self.get_prototype_of(&current)?;
        }
        Err(crate::value::limit_err(
            "Maximum prototype chain depth exceeded",
        ))
    }

    pub(super) fn destructure(&mut self, pat: &Pattern, val: &Value) -> Result<Value, VmErr> {
        match pat {
            Pattern::Ident(name) => {
                // A *declaration* pre-declares its names in this scope, still
                // in their temporal dead zone; giving one its value is an
                // initialization, which `assign` would refuse. Anything else
                // is a destructuring *assignment*, which must reach the
                // binding it names wherever that is.
                if self.global.borrow_mut().initialize(name, val.clone()) {
                    return Ok(val.clone());
                }
                self.assign_or_set_binding(name, val.clone())?;
                Ok(val.clone())
            }
            Pattern::Member { object, property } => {
                let receiver = self.eval_expr(object)?;
                let key = self.eval_expr(property)?;
                self.assign_member(&receiver, &key, val.clone())?;
                Ok(val.clone())
            }
            Pattern::Array(elements) => {
                let values: Vec<Value> = match val {
                    Value::Array(arr) => arr.borrow().clone(),
                    // Plain objects are not iterable and must never be
                    // materialized into a sparse vector keyed by guest data.
                    // The old numeric-key path let `{ "1000000000": 1 }`
                    // request an enormous allocation during destructuring.
                    Value::Object { .. } => vec![],
                    Value::String(s) => {
                        if s.chars().count() > crate::value::MAX_ARRAY_LEN {
                            return Err(crate::value::limit_err("Maximum array length exceeded"));
                        }
                        s.chars().map(|c| Value::String(c.to_string())).collect()
                    }
                    _ => vec![],
                };
                let mut rest_target = None;
                for (i, elem) in elements.iter().enumerate() {
                    if elem.is_rest() {
                        rest_target = Some(i);
                        break;
                    }
                    if let Some(v) = values.get(i) {
                        self.destructure(elem, v)?;
                    } else {
                        self.destructure(elem, &Value::Undefined)?;
                    }
                }
                if let Some(rest_idx) = rest_target
                    && let Pattern::Rest(rest_pat) = &elements[rest_idx]
                {
                    let rest_vals = values[rest_idx..].to_vec();
                    let rest_val = Value::array(rest_vals);
                    self.destructure(rest_pat, &rest_val)?;
                }
                Ok(val.clone())
            }
            Pattern::Object(props) => {
                let obj: Vec<(String, Value)> = match val {
                    Value::Object { props: oprops, .. } => oprops.borrow().clone(),
                    _ => vec![],
                };
                let mut taken: Vec<&str> = Vec::new();
                for (key, pat) in props {
                    // `{ ...rest }` takes whatever the named keys did not.
                    if key == "..."
                        && let Some(Pattern::Rest(target)) = pat
                    {
                        let remaining: Vec<(String, Value)> = obj
                            .iter()
                            .filter(|(k, _)| {
                                !taken.contains(&k.as_str())
                                    && !crate::interpreter::is_internal_key(k)
                            })
                            .cloned()
                            .collect();
                        let rest = Value::checked_object(remaining)?;
                        self.destructure(target, &rest)?;
                        continue;
                    }
                    taken.push(key);
                    let mut found = Value::Undefined;
                    for (k, v) in &obj {
                        if k == key {
                            found = v.clone();
                            break;
                        }
                    }
                    if let Some(p) = pat {
                        self.destructure(p, &found)?;
                    } else {
                        self.set_binding(key, found)?;
                    }
                }
                Ok(val.clone())
            }
            Pattern::Rest(_) => Ok(val.clone()),
            Pattern::Default(pat, default_expr) => {
                if matches!(val, Value::Undefined | Value::Null) {
                    let default_val = self.eval_expr(default_expr)?;
                    self.destructure(pat, &default_val)
                } else {
                    self.destructure(pat, val)
                }
            }
        }
    }

    pub(super) fn run_catch(
        &mut self,
        catch: &Option<(String, Vec<Statement>)>,
        err_val: Value,
    ) -> Result<Value, VmErr> {
        if let Some((p, cb)) = catch {
            let ce = Rc::new(RefCell::new(Environment::child(self.global.clone())));
            ce.borrow_mut().set(p, err_val);
            // The catch parameter lives in its own scope, and the catch block
            // is a block: its lexical declarations belong to that scope too.
            let s = std::mem::replace(&mut self.global, ce);
            // Only lexical hoisting here: a `var` inside `catch` belongs to
            // the enclosing function scope, where it was already hoisted.
            let r = self.run_hoisted_here(cb);
            self.global = s;
            r
        } else {
            // No catch clause: re-throw the original value.
            Err(VmErr::Throw(err_val))
        }
    }

    /// `delete obj[key]`: remove an own property and report whether the
    /// object is left without it.
    ///
    /// Non-configurable properties survive and yield `false`; a missing
    /// property is already absent, so deleting it succeeds.
    pub(crate) fn delete_member(&mut self, obj: &Value, key: &Value) -> Result<Value, VmErr> {
        if let Some(proxy) = obj.as_proxy() {
            let target = proxy.target.clone();
            if let Some(trap) = self.proxy_trap(&proxy, "deleteProperty") {
                let name = self.proxy_property_key(key)?;
                let handler = proxy.handler.clone();
                let result = self.call_this(&trap, handler, vec![target, name])?;
                return Ok(Value::Bool(result.is_truthy()));
            }
            return self.delete_member(&target, key);
        }
        match obj {
            Value::Object { props } => {
                let slot = self.property_key(key)?;
                if !props.meta.borrow().attrs_of(&slot).configurable
                    && props.borrow().iter().any(|(k, _)| *k == slot)
                {
                    return Ok(Value::Bool(false));
                }
                let mut slots = props.borrow_mut();
                if let Some(index) = slots.iter().position(|(k, _)| *k == slot) {
                    slots.remove(index);
                    drop(slots);
                    props.meta.borrow_mut().forget(&slot);
                }
                Ok(Value::Bool(true))
            }
            Value::Class(class) => {
                let slot = self.property_key(key)?;
                if !class.statics.meta.borrow().attrs_of(&slot).configurable
                    && class.statics.borrow().iter().any(|(name, _)| name == &slot)
                {
                    return Ok(Value::Bool(false));
                }
                let companion = format!("__setter:{}__", slot);
                class
                    .statics
                    .borrow_mut()
                    .retain(|(name, _)| name != &slot && name != &companion);
                class.statics.meta.borrow_mut().forget(&slot);
                class.statics.meta.borrow_mut().forget(&companion);
                Ok(Value::Bool(true))
            }
            Value::Function(function) => {
                let slot = self.property_key(key)?;
                function.ensure_name_length_properties();
                function.prototype_value(obj);
                if !function
                    .properties
                    .meta
                    .borrow()
                    .attrs_of(&slot)
                    .configurable
                    && function
                        .properties
                        .borrow()
                        .iter()
                        .any(|(name, _)| name == &slot)
                {
                    return Ok(Value::Bool(false));
                }
                let companion = format!("__setter:{}__", slot);
                function
                    .properties
                    .borrow_mut()
                    .retain(|(name, _)| name != &slot && name != &companion);
                function.properties.meta.borrow_mut().forget(&slot);
                function.properties.meta.borrow_mut().forget(&companion);
                Ok(Value::Bool(true))
            }
            Value::HostFunction { properties, .. } => {
                let slot = self.property_key(key)?;
                if !properties.meta.borrow().attrs_of(&slot).configurable
                    && properties.borrow().iter().any(|(name, _)| name == &slot)
                {
                    return Ok(Value::Bool(false));
                }
                let companion = format!("__setter:{}__", slot);
                properties
                    .borrow_mut()
                    .retain(|(name, _)| name != &slot && name != &companion);
                properties.meta.borrow_mut().forget(&slot);
                properties.meta.borrow_mut().forget(&companion);
                Ok(Value::Bool(true))
            }
            // Deleting an array element leaves an absent slot while reads
            // continue to produce `undefined`.
            Value::Array(items) => {
                let property = self.property_key(key)?;
                if property == "length" {
                    return Ok(Value::Bool(false));
                }
                if let Some(index) = crate::value::array_index(&property) {
                    let items_len = items.borrow().len();
                    if index < items_len && items.has_index(index) {
                        if !items.meta.borrow().attrs_of(&property).configurable {
                            return Ok(Value::Bool(false));
                        }
                        items.borrow_mut()[index] = Value::Undefined;
                        items.set_index_presence(index, false);
                        let companion = format!("__setter:{}__", property);
                        items
                            .named
                            .borrow_mut()
                            .retain(|(name, _)| name != &companion);
                    }
                } else {
                    if items.named_prop(&property).is_some()
                        && !items.meta.borrow().attrs_of(&property).configurable
                    {
                        return Ok(Value::Bool(false));
                    }
                    items.named.borrow_mut().retain(|(name, _)| {
                        name != &property && name != &format!("__setter:{}__", property)
                    });
                    items.forget_symbol_key(&property);
                }
                Ok(Value::Bool(true))
            }
            _ => Ok(Value::Bool(true)),
        }
    }

    /// Coerce a value used in a computed member expression to the slot name
    /// the object stores it under.
    pub(crate) fn property_key(&self, key: &Value) -> Result<String, VmErr> {
        Ok(match key {
            Value::String(k) => k.clone(),
            Value::Number(n) => crate::format::number_string(*n),
            Value::Symbol(s) => crate::interpreter::symbol_slot_key(s),
            other => self.vs(other)?,
        })
    }

    fn assign_cell_property(
        &mut self,
        receiver: &Value,
        props: &ObjectCell,
        key: &str,
        value: Value,
    ) -> Result<(), VmErr> {
        let setter_name = format!("set {key}");
        let getter_name = format!("get {key}");
        let is_setter = |value: &Value| match value {
            Value::Function(function) => function.name.as_deref() == Some(setter_name.as_str()),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.as_ref() == setter_name
            }
            _ => false,
        };
        let is_getter = |value: &Value| match value {
            Value::Function(function) => function.name.as_deref() == Some(getter_name.as_str()),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.as_ref() == getter_name
            }
            _ => false,
        };
        let existing = {
            let slots = props.borrow();
            slots.iter().position(|(name, _)| name == key).map(|index| {
                let property = &slots[index].1;
                (
                    index,
                    is_setter(property).then(|| property.clone()),
                    is_getter(property),
                )
            })
        };
        if let Some((_, Some(setter), _)) = &existing {
            self.call_this(setter, receiver.clone(), vec![value])?;
            return Ok(());
        }
        if props.meta.borrow().has_accessors {
            let companion = format!("__setter:{}__", key);
            let setter = props
                .borrow()
                .iter()
                .find(|(name, value)| (name == key || name == &companion) && is_setter(value))
                .map(|(_, value)| value.clone());
            if let Some(setter) = setter {
                self.call_this(&setter, receiver.clone(), vec![value])?;
                return Ok(());
            }
        }
        if let Some((_, _, true)) = existing {
            // A getter without a setter is an accessor, not a writable data
            // property. Accessors with a setter returned above.
            return Ok(());
        }
        if let Some((index, _, _)) = existing {
            if props.meta.borrow().attrs_of(key).writable {
                props.borrow_mut()[index].1 = value;
            }
            return Ok(());
        }
        if let Some(prototype) = self.prototype_of(receiver)
            && self.assign_inherited_property(receiver, prototype.as_ref(), key, &value)?
        {
            return Ok(());
        }
        if props.meta.borrow().non_extensible {
            return Ok(());
        }
        let mut slots = props.borrow_mut();
        if slots.len() >= crate::value::MAX_OBJECT_PROPS {
            return Err(crate::value::limit_err(
                "Maximum object property count exceeded",
            ));
        }
        slots.push((key.to_owned(), value));
        Ok(())
    }

    /// Apply the `[[Set]]` behavior for a property found on the prototype
    /// chain. Inherited accessors receive the original object as `this`,
    /// inherited getter-only and non-writable properties block creation of an
    /// own property, and inherited writable data properties allow it.
    fn assign_inherited_property(
        &mut self,
        receiver: &Value,
        prototype: &Value,
        key: &str,
        value: &Value,
    ) -> Result<bool, VmErr> {
        let setter_name = format!("set {key}");
        let getter_name = format!("get {key}");
        let is_setter = |value: &Value| match value {
            Value::Function(function) => function.name.as_deref() == Some(setter_name.as_str()),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.as_ref() == setter_name
            }
            _ => false,
        };
        let is_getter = |value: &Value| match value {
            Value::Function(function) => function.name.as_deref() == Some(getter_name.as_str()),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.as_ref() == getter_name
            }
            _ => false,
        };

        let mut current = prototype.clone();
        for _ in 0..=crate::value::MAX_PROTOTYPE_DEPTH {
            let (slots, attributes, has_accessors) = match &current {
                Value::Object { props } => {
                    let meta = props.meta.borrow();
                    (
                        props.borrow().clone(),
                        meta.attrs_of(key),
                        meta.has_accessors,
                    )
                }
                Value::Class(class) => {
                    let meta = class.statics.meta.borrow();
                    (
                        class.statics.borrow().clone(),
                        meta.attrs_of(key),
                        meta.has_accessors,
                    )
                }
                Value::Array(array) => {
                    let meta = array.meta.borrow();
                    (
                        array.named.borrow().clone(),
                        meta.attrs_of(key),
                        meta.has_accessors,
                    )
                }
                _ => return Ok(false),
            };
            if let Some((_, property)) = slots.iter().find(|(name, _)| name == key) {
                let companion = format!("__setter:{}__", key);
                let paired_setter = has_accessors.then(|| {
                    slots
                        .iter()
                        .find(|(name, candidate)| name == &companion && is_setter(candidate))
                        .map(|(_, setter)| setter.clone())
                });
                let setter = if is_setter(property) {
                    Some(property.clone())
                } else {
                    paired_setter.flatten()
                };
                if let Some(setter) = setter {
                    self.call_this(&setter, receiver.clone(), vec![value.clone()])?;
                    return Ok(true);
                }
                if is_getter(property)
                    || has_accessors && slots.iter().any(|(name, _)| name == &companion)
                {
                    return Ok(true);
                }
                return Ok(!attributes.writable);
            }
            let Some(next) = self.prototype_of(&current) else {
                return Ok(false);
            };
            current = next.as_ref().clone();
        }
        Ok(false)
    }

    pub(crate) fn assign_member(
        &mut self,
        obj: &Value,
        prop: &Value,
        val: Value,
    ) -> Result<(), VmErr> {
        // A proxy's `set` trap replaces the write; without one it falls
        // through to the target.
        if let Some(proxy) = obj.as_proxy() {
            let target = proxy.target.clone();
            if let Some(trap) = self.proxy_trap(&proxy, "set") {
                let key = self.proxy_property_key(prop)?;
                let handler = proxy.handler.clone();
                self.call_this(&trap, handler, vec![target, key, val, obj.clone()])?;
                return Ok(());
            }
            return self.assign_member(&target, prop, val);
        }
        match (obj, prop) {
            (Value::Function(function), Value::Symbol(symbol)) => {
                function.ensure_name_length_properties();
                function.prototype_value(obj);
                let slot = crate::interpreter::symbol_slot_key(symbol);
                self.assign_cell_property(obj, &function.properties, &slot, val)?;
                function
                    .properties
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&slot, symbol.clone());
                Ok(())
            }
            (Value::Function(function), Value::String(key)) => {
                function.ensure_name_length_properties();
                function.prototype_value(obj);
                self.assign_cell_property(obj, &function.properties, key, val)
            }
            (Value::Function(_), _) => {
                let slot = self.property_key(prop)?;
                self.assign_member(obj, &Value::String(slot), val)
            }
            (Value::HostFunction { properties, .. }, Value::Symbol(symbol)) => {
                let slot = crate::interpreter::symbol_slot_key(symbol);
                self.assign_cell_property(obj, properties, &slot, val)?;
                properties
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&slot, symbol.clone());
                Ok(())
            }
            (Value::HostFunction { properties, .. }, Value::String(key)) => {
                self.assign_cell_property(obj, properties, key, val)
            }
            (Value::HostFunction { .. }, _) => {
                let slot = self.property_key(prop)?;
                self.assign_member(obj, &Value::String(slot), val)
            }
            (Value::Object { props }, Value::Symbol(symbol)) => {
                let slot = crate::interpreter::symbol_slot_key(symbol);
                self.assign_cell_property(obj, props, &slot, val)?;
                props
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&slot, symbol.clone());
                Ok(())
            }
            (Value::Object { props }, Value::String(k)) => {
                self.assign_cell_property(obj, props, k, val)
            }
            // Any other key on an object is coerced to its slot name first:
            // `o[1] = v`, `o[sym] = v`, `o[{}] = v`.
            (Value::Object { .. }, _) => {
                let slot = self.property_key(prop)?;
                self.assign_member(obj, &Value::String(slot), val)
            }
            (Value::Class(class), Value::Symbol(symbol)) => {
                let slot = crate::interpreter::symbol_slot_key(symbol);
                self.assign_cell_property(obj, &class.statics, &slot, val)?;
                class
                    .statics
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&slot, symbol.clone());
                Ok(())
            }
            (Value::Class(class), Value::String(k)) => {
                self.assign_cell_property(obj, &class.statics, k, val)
            }
            (Value::Class(_), _) => {
                let slot = self.property_key(prop)?;
                self.assign_member(obj, &Value::String(slot), val)
            }
            // Writing an element of a typed array converts and wraps it to
            // the element type; an out-of-range index is ignored, not grown.
            (Value::TypedArray(view), key) => {
                let index = self.tn(key);
                if index.is_finite() && index >= 0.0 && index.fract() == 0.0 {
                    crate::builtins::write_element(view, index as usize, &val)?;
                }
                Ok(())
            }
            // `re.lastIndex = 0` resets a global pattern's scan position.
            (Value::RegExp(data), Value::String(k)) if k == "lastIndex" => {
                let index = self.tn(&val);
                data.last_index.set(if index.is_finite() && index > 0.0 {
                    index as usize
                } else {
                    0
                });
                Ok(())
            }
            // `window.x = v` / `globalThis.x = v` define a real global.
            (Value::GlobalObject, Value::String(k)) => self.set_global_checked(k, val),
            // A non-index key on an array is a named property, not an
            // element: `strings.raw`, `arr.total = 3`.
            (Value::Array(cell), Value::String(k))
                if k != "length" && crate::value::array_index(k).is_none() =>
            {
                let exists = cell.named_prop(k).is_some();
                if cell.meta.borrow().has_accessors {
                    let current = cell.named_prop(k);
                    if let Some(setter) = array_property_setter(cell, k, current.as_ref()) {
                        self.call_this(&setter, obj.clone(), vec![val])?;
                        return Ok(());
                    }
                }
                if !exists
                    && let Some(prototype) = self.prototype_of(obj)
                    && self.assign_inherited_property(obj, prototype.as_ref(), k, &val)?
                {
                    return Ok(());
                }
                if (exists && !cell.meta.borrow().attrs_of(k).writable)
                    || (!exists && cell.meta.borrow().non_extensible)
                {
                    return Ok(());
                }
                cell.set_named(k.clone(), val);
                Ok(())
            }
            (Value::Array(cell), Value::Symbol(symbol)) => {
                let slot = crate::interpreter::symbol_slot_key(symbol);
                let exists = cell.named_prop(&slot).is_some();
                if cell.meta.borrow().has_accessors {
                    let current = cell.named_prop(&slot);
                    if let Some(setter) = array_property_setter(cell, &slot, current.as_ref()) {
                        self.call_this(&setter, obj.clone(), vec![val])?;
                        return Ok(());
                    }
                }
                if !exists
                    && let Some(prototype) = self.prototype_of(obj)
                    && self.assign_inherited_property(obj, prototype.as_ref(), &slot, &val)?
                {
                    return Ok(());
                }
                if (exists && !cell.meta.borrow().attrs_of(&slot).writable)
                    || (!exists && cell.meta.borrow().non_extensible)
                {
                    return Ok(());
                }
                cell.set_named(slot.clone(), val);
                cell.set_symbol_key(&slot, symbol.clone());
                Ok(())
            }
            (Value::Array(cell), Value::String(k)) if k == "length" => {
                let length = self.tn(&val);
                if cell.meta.borrow().attrs_of("length").writable
                    && length.is_finite()
                    && length >= 0.0
                    && length.fract() == 0.0
                {
                    let length = (length as usize).min(crate::value::MAX_ARRAY_LEN);
                    cell.set_length(length);
                }
                Ok(())
            }
            (Value::Array(_), Value::String(k)) => {
                let index = crate::value::array_index(k).expect("canonical array index guard");
                self.assign_member(obj, &Value::Number(index as f64), val)
            }
            (Value::Array(items), Value::Number(i)) => {
                if !i.is_finite() || *i < 0.0 || i.fract() != 0.0 {
                    return Err(VmErr::Msg("TypeError: Invalid array index".to_string()));
                }
                if *i >= crate::value::MAX_ARRAY_LEN as f64 {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                let idx = *i as usize;
                let old_length = items.borrow().len();
                let exists = idx < old_length && items.has_index(idx);
                let key = idx.to_string();
                if exists && items.meta.borrow().has_accessors {
                    let current = items.borrow().get(idx).cloned();
                    if let Some(setter) = array_property_setter(items, &key, current.as_ref()) {
                        self.call_this(&setter, obj.clone(), vec![val])?;
                        return Ok(());
                    }
                }
                let attributes = items.meta.borrow().attrs_of(&key);
                if (exists && !attributes.writable)
                    || (!exists && items.meta.borrow().non_extensible)
                    || (idx >= old_length && !items.meta.borrow().attrs_of("length").writable)
                {
                    return Ok(());
                }
                let mut items = items.borrow_mut();
                if idx < items.len() {
                    items[idx] = val;
                } else {
                    // `resize` performs the bounded growth without a
                    // guest-visible native loop or an unchecked index.
                    items.resize(idx, Value::Undefined);
                    items.push(val);
                }
                let new_length = items.len();
                drop(items);
                if let Value::Array(cell) = obj {
                    if idx >= old_length {
                        cell.resize_presence(old_length, new_length, false);
                    }
                    cell.set_index_presence(idx, true);
                }
                Ok(())
            }
            _ => Err(VmErr::Msg("Invalid assignment target".to_string())),
        }
    }

    pub(crate) fn call_this(
        &mut self,
        f: &Value,
        this_val: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        if args.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum argument count exceeded"));
        }
        match f {
            Value::Function(fd) => {
                if let Some(bound) = &fd.bound {
                    let bound = bound.clone();
                    if bound.arguments.len().saturating_add(args.len())
                        > crate::value::MAX_ARRAY_LEN
                    {
                        return Err(crate::value::limit_err("Maximum argument count exceeded"));
                    }
                    let mut call_args = bound.arguments.as_ref().clone();
                    call_args.extend(args);
                    return self.call_this(&bound.target, bound.this_value.clone(), call_args);
                }
                // Calling a generator function does not run its body; it returns
                // a generator object whose `next()` method drives execution.
                if fd.is_generator {
                    let inner = GeneratorInner {
                        body: fd.body.clone(),
                        closure: fd.closure.clone(),
                        params: fd.params.clone(),
                        args,
                        #[cfg(stackful_coroutines)]
                        coroutine: None,
                        #[cfg(not(stackful_coroutines))]
                        buffered: std::collections::VecDeque::new(),
                        started: false,
                        done: false,
                        return_value: None,
                    };
                    return Ok(Value::Generator {
                        inner: Rc::new(RefCell::new(inner)),
                    });
                }
                // Recursion guard: each VM call costs several native frames,
                // so unbounded guest recursion would SIGSEGV the host. Fail
                // with a catchable RangeError instead (V8 semantics).
                if self.get_stack().len() >= crate::interpreter::MAX_CALL_DEPTH {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum call stack size exceeded".to_string(),
                    ));
                }
                let parent_env = fd.closure.clone().unwrap_or_else(|| self.global.clone());
                let rest_idx = fd.params.iter().position(|p| p.starts_with("..."));
                let fe = match rest_idx {
                    // Fast path (the overwhelming majority of calls): no rest
                    // parameter. Build the whole frame — `this`, params, and
                    // the optional `arguments` object — as one binding list
                    // and allocate the environment exactly once. No
                    // per-parameter `RefCell` borrows, no insertion scans.
                    None => {
                        let mut vars: SmallVec<[(Key, Value); 8]> = SmallVec::new();
                        // Regular functions bind their own `this`; arrows
                        // inherit the enclosing lexical `this` through the
                        // closure chain.
                        if !fd.is_arrow {
                            vars.push((Key::from("this"), this_val));
                        }
                        for (i, p) in fd.params.iter().enumerate() {
                            let arg = args.get(i).cloned().unwrap_or(Value::Undefined);
                            vars.push((p.clone(), arg));
                        }
                        // Create the (detached) arguments object only when
                        // the body actually reads it; most functions never do.
                        if fd.uses_arguments {
                            let args_obj = Value::object(
                                args.iter()
                                    .enumerate()
                                    .map(|(i, v)| (i.to_string(), v.clone()))
                                    .collect(),
                            );
                            args_obj
                                .set_prop("length".to_string(), Value::Number(args.len() as f64))?;
                            vars.push((Key::from("arguments"), args_obj));
                        }
                        Rc::new(RefCell::new(Environment::with_bindings(parent_env, vars)))
                    }
                    // Slow path: rest parameters need positional fixups that
                    // are not worth special-casing into the batch builder.
                    Some(rest_idx) => {
                        let fe = Rc::new(RefCell::new(Environment::child(parent_env)));
                        if !fd.is_arrow {
                            fe.borrow_mut().set("this", this_val);
                        }
                        let rest_name = fd.params[rest_idx].trim_start_matches("...").to_string();
                        for (i, p) in fd.params.iter().enumerate() {
                            if i == rest_idx {
                                let rest_args = args[i..].to_vec();
                                fe.borrow_mut().set(&rest_name, Value::array(rest_args));
                            } else {
                                let arg = if i < args.len() {
                                    let is_rest_param = fd
                                        .params
                                        .get(i + 1)
                                        .map(|p| p.starts_with("..."))
                                        .unwrap_or(false);
                                    if !is_rest_param && i >= rest_idx {
                                        Value::Undefined
                                    } else {
                                        args.get(i).cloned().unwrap_or(Value::Undefined)
                                    }
                                } else {
                                    Value::Undefined
                                };
                                fe.borrow_mut().set(p, arg);
                            }
                        }
                        if fd.uses_arguments {
                            let args_obj = Value::object(
                                args.iter()
                                    .enumerate()
                                    .map(|(i, v)| (i.to_string(), v.clone()))
                                    .collect(),
                            );
                            args_obj
                                .set_prop("length".to_string(), Value::Number(args.len() as f64))?;
                            fe.borrow_mut().set("arguments", args_obj);
                        }
                        fe
                    }
                };

                // An async body runs on its own stack so `await` can suspend
                // it. The frame is already built, so the coroutine starts
                // straight into the body.
                #[cfg(stackful_coroutines)]
                if fd.is_async {
                    if self.gen_depth >= crate::interpreter::MAX_GENERATOR_DEPTH {
                        return Err(crate::value::limit_err("Maximum async nesting exceeded"));
                    }
                    return super::async_fn::spawn_async(
                        self,
                        fd.body.clone(),
                        fe,
                        self.gen_depth + 1,
                    );
                }

                let s = std::mem::replace(&mut self.global, fe);
                // `name` is an `Rc<str>`: cloning it for the stack frame is a
                // refcount bump, so the hot path allocates nothing here.
                let fname = fd
                    .name
                    .clone()
                    .unwrap_or_else(|| Rc::<str>::from("<anonymous>"));
                self.push_frame(fname, Span::unknown());
                // A function body is a fresh variable scope: `var` and
                // function declarations hoist to it, lexical ones dead-zone.
                let r = self.run_program_body(&fd.body);
                // Convert a bare message into a located runtime error *before*
                // popping the frame, so the snapshot carries the full call
                // chain. Only the error path pays for the snapshot — the
                // success path (the overwhelming majority of calls) clones
                // nothing. (Snapshotting unconditionally here was the single
                // largest per-call cost: O(depth) String clones per call.)
                let result = match r {
                    Err(VmErr::Ret(v)) => Ok(v),
                    Err(VmErr::Msg(msg)) => Err(VmErr::RuntimeError(Box::new(RuntimeErrorData {
                        message: msg,
                        span: None,
                        stack: self.get_stack().to_vec(),
                    }))),
                    other => other,
                };
                self.pop_frame();
                self.global = s;
                if fd.is_async {
                    // An async function always resolves to a promise.
                    match result {
                        Ok(v) => {
                            let promise = Value::pending_promise();
                            self.resolve_promise(&promise, v)?;
                            Ok(Value::Promise(promise))
                        }
                        Err(VmErr::Throw(v)) => {
                            Ok(Value::settled_promise(PromiseState::Rejected, v))
                        }
                        other => other,
                    }
                } else {
                    result
                }
            }
            Value::NativeFunction { callable, .. } => callable(self, this_val, args),
            Value::HostFunction { properties, .. } => {
                let id = properties
                    .meta
                    .borrow()
                    .host_function_id
                    .expect("host function identity is initialized");
                // Clone the bridge out so we don't hold a borrow on `self`
                // across the host call (which may re-enter the VM).
                let bridge = self.host.clone().ok_or_else(|| {
                    VmErr::Msg("cannot call host function: no bridge attached".to_string())
                })?;
                if bridge.is_async_fn(id) {
                    // Async host function: dispatch the call and return a
                    // pending sentinel. The interpreter parks at `await`.
                    bridge.call_host_async_with_this(id, this_val, args)
                } else {
                    bridge.call_host_with_callback_handler(id, this_val, args, &mut |callback| {
                        self.run_host_callback(callback)
                    })
                }
            }
            // A proxy over a function: `apply` intercepts the call.
            Value::Proxy(proxy) => {
                let target = proxy.target.clone();
                match self.proxy_trap(&proxy.clone(), "apply") {
                    Some(trap) => {
                        let handler = proxy.handler.clone();
                        let arg_list = Value::checked_array(args)?;
                        self.call_this(&trap, handler, vec![target, this_val, arg_list])
                    }
                    None => self.call_this(&target, this_val, args),
                }
            }
            // A built-in namespace such as `String` or `Map` is an object
            // carrying its statics *and* an internal call slot, so it can be
            // both `String.fromCharCode(…)` and `String(x)`.
            Value::Object { .. } => match callable_slot(f, CALL_SLOT) {
                // The object itself becomes the receiver when the call site
                // supplied none. A native function is a bare pointer with
                // nowhere to keep state, so the built-ins that need to carry
                // something — a promise's `resolve`, a combinator's slot index
                // — keep it in a hidden property and read it off `this`.
                Some(target) => {
                    let receiver = if matches!(this_val, Value::Undefined) {
                        f.clone()
                    } else {
                        this_val
                    };
                    self.call_this(&target, receiver, args)
                }
                None => vm_err("TypeError: object is not a function".to_string()),
            },
            _ => {
                let type_name = match f {
                    Value::String(_) => "string",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    Value::Null => "null",
                    Value::Undefined => "undefined",
                    Value::Array(_) => "array",
                    _ => "unknown",
                };
                vm_err(format!("TypeError: {} is not a function", type_name))
            }
        }
    }

    pub(crate) fn run_host_callback(
        &mut self,
        callback: crate::host::HostCallback,
    ) -> Result<Value, VmErr> {
        match callback.kind {
            crate::host::HostCallbackKind::Call => {
                self.call_this(&callback.callback, callback.this_value, callback.args)
            }
            crate::host::HostCallbackKind::MakeCallback => {
                // Node-API uses napi_make_callback both from native async
                // completions and from synchronous native calls. A callback
                // made while guest JavaScript is already on the stack must
                // leave its microtasks for the enclosing stack checkpoint.
                let should_checkpoint = self.guest_execution_depth.get() == 0;
                let result = self.call_this(&callback.callback, callback.this_value, callback.args);
                if should_checkpoint {
                    let checkpoint = self.drain_microtasks();
                    match result {
                        Err(error) => {
                            let _ = checkpoint;
                            Err(error)
                        }
                        Ok(value) => {
                            checkpoint?;
                            Ok(value)
                        }
                    }
                } else {
                    result
                }
            }
            crate::host::HostCallbackKind::Construct => {
                self.ctor(&callback.callback, callback.args)
            }
        }
    }

    /// Run a constructor (class or function) against an already-created `this`,
    /// as done by `super(...)`. Returns `this`.
    pub(super) fn invoke_ctor(
        &mut self,
        f: &Value,
        this_val: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        let new_target = self
            .new_target_stack
            .last()
            .cloned()
            .unwrap_or_else(|| f.clone());
        self.invoke_constructor_with_new_target(f, this_val.clone(), args, new_target)?;
        Ok(this_val)
    }

    fn invoke_constructor_with_new_target(
        &mut self,
        f: &Value,
        this_val: Value,
        args: Vec<Value>,
        new_target: Value,
    ) -> Result<Value, VmErr> {
        match f {
            Value::Class(c) => self.invoke_constructor_with_new_target(
                c.constructor.as_ref(),
                this_val,
                args,
                new_target,
            ),
            Value::Function(function) => {
                if let Some(bound) = &function.bound {
                    if !function.is_constructor {
                        return vm_err("TypeError: function is not a constructor");
                    }
                    let bound = bound.clone();
                    if bound.arguments.len().saturating_add(args.len())
                        > crate::value::MAX_ARRAY_LEN
                    {
                        return Err(crate::value::limit_err("Maximum argument count exceeded"));
                    }
                    let mut call_args = bound.arguments.as_ref().clone();
                    call_args.extend(args);
                    self.invoke_constructor_with_new_target(
                        &bound.target,
                        this_val,
                        call_args,
                        new_target,
                    )
                } else {
                    self.call_this(f, this_val, args)
                }
            }
            // The built-in error types have native constructors, so
            // `class E extends Error {}` reaches `super(…)` here.
            Value::NativeFunction { .. } => self.call_this(f, this_val, args),
            Value::HostFunction { properties, .. } => {
                let id = properties
                    .meta
                    .borrow()
                    .host_function_id
                    .expect("host function identity is initialized");
                let bridge = self.host.clone().ok_or_else(|| {
                    VmErr::Msg("cannot construct host function: no bridge attached".to_string())
                })?;
                bridge.call_host_constructor_with_callback_handler_and_target(
                    id,
                    this_val,
                    args,
                    new_target,
                    &mut |callback| self.run_host_callback(callback),
                )
            }
            _ => {
                let type_name = match f {
                    Value::String(_) => "string",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    Value::Null => "null",
                    Value::Undefined => "undefined",
                    Value::Array(_) => "array",
                    Value::Object { .. } => "object",
                    _ => "unknown",
                };
                vm_err(format!("TypeError: {} is not a constructor", type_name))
            }
        }
    }

    pub(crate) fn ctor(&mut self, f: &Value, args: Vec<Value>) -> Result<Value, VmErr> {
        let new_target = f.clone();
        self.new_target_stack.push(new_target.clone());
        let result = self.ctor_with_new_target(f, args, new_target);
        self.new_target_stack.pop();
        result
    }

    fn ctor_with_new_target(
        &mut self,
        f: &Value,
        args: Vec<Value>,
        new_target: Value,
    ) -> Result<Value, VmErr> {
        // A built-in namespace object constructs through its internal slot:
        // `new Map()` and `Map()` reach the same implementation unless the
        // built-in installs a separate one.
        if let Value::Object { .. } = f
            && let Some(target) =
                callable_slot(f, CONSTRUCT_SLOT).or_else(|| callable_slot(f, CALL_SLOT))
        {
            // The namespace object is the receiver, so a shared implementation
            // can tell which built-in it was reached through — `Int8Array` and
            // `Float64Array` differ only by what their namespace carries.
            return self.call_this(&target, f.clone(), args);
        }
        if let Some(proxy) = f.as_proxy() {
            let target = proxy.target.clone();
            return match self.proxy_trap(&proxy, "construct") {
                Some(trap) => {
                    let handler = proxy.handler.clone();
                    let arg_list = Value::checked_array(args)?;
                    self.call_this(&trap, handler, vec![target, arg_list, new_target])
                }
                None => self.ctor_with_new_target(&target, args, new_target),
            };
        }
        match f {
            Value::HostFunction { properties, .. } => {
                let id = properties
                    .meta
                    .borrow()
                    .host_function_id
                    .expect("host function identity is initialized");
                let instance = Value::object(vec![]);
                let bridge = self.host.clone().ok_or_else(|| {
                    VmErr::Msg("cannot construct host function: no bridge attached".to_string())
                })?;
                let result = bridge.construct_host_with_callback_handler_and_target(
                    id,
                    instance.clone(),
                    args,
                    new_target,
                    &mut |callback| self.run_host_callback(callback),
                )?;
                if is_js_object(&result) {
                    Ok(result)
                } else {
                    Ok(instance)
                }
            }
            Value::Class(c) => {
                // The instance's prototype is the class prototype (shared Rc, so
                // `instanceof` can compare identity).
                let inst = Value::object_with_proto(vec![], Some(c.prototype.clone()));
                let r = self.invoke_constructor_with_new_target(
                    c.constructor.as_ref(),
                    inst.clone(),
                    args,
                    new_target,
                )?;
                if is_js_object(&r) { Ok(r) } else { Ok(inst) }
            }
            Value::Function(fd) => {
                if let Some(bound) = &fd.bound {
                    if !fd.is_constructor {
                        return vm_err("TypeError: function is not a constructor");
                    }
                    let bound = bound.clone();
                    if bound.arguments.len().saturating_add(args.len())
                        > crate::value::MAX_ARRAY_LEN
                    {
                        return Err(crate::value::limit_err("Maximum argument count exceeded"));
                    }
                    let mut call_args = bound.arguments.as_ref().clone();
                    call_args.extend(args);
                    let new_target = if crate::interpreter::strict_equals(&new_target, f) {
                        bound.target.clone()
                    } else {
                        new_target
                    };
                    return self.ctor_with_new_target(&bound.target, call_args, new_target);
                }
                if !fd.is_constructor {
                    return vm_err("TypeError: function is not a constructor");
                }
                let prototype =
                    self.get_prop_value(&new_target, &Value::String("prototype".to_string()))?;
                let inst = if is_js_object(&prototype) {
                    Value::object_with_proto(vec![], Some(Rc::new(prototype)))
                } else {
                    Value::object(vec![])
                };
                let parent_env = fd.closure.clone().unwrap_or_else(|| self.global.clone());

                let rest_idx = fd.params.iter().position(|p| p.starts_with("..."));
                let fe = match rest_idx {
                    None => {
                        let mut vars: SmallVec<[(Key, Value); 8]> = SmallVec::new();
                        vars.push((Key::from("this"), inst.clone()));
                        for (i, p) in fd.params.iter().enumerate() {
                            let arg = args.get(i).cloned().unwrap_or(Value::Undefined);
                            vars.push((p.clone(), arg));
                        }
                        if fd.uses_arguments {
                            let args_obj = Value::object(
                                args.iter()
                                    .enumerate()
                                    .map(|(i, v)| (i.to_string(), v.clone()))
                                    .collect(),
                            );
                            args_obj
                                .set_prop("length".to_string(), Value::Number(args.len() as f64))?;
                            vars.push((Key::from("arguments"), args_obj));
                        }
                        Rc::new(RefCell::new(Environment::with_bindings(parent_env, vars)))
                    }
                    Some(rest_idx) => {
                        let fe = Rc::new(RefCell::new(Environment::child(parent_env)));
                        fe.borrow_mut().set("this", inst.clone());
                        let rest_name = fd.params[rest_idx].trim_start_matches("...").to_string();
                        for (i, p) in fd.params.iter().enumerate() {
                            if i == rest_idx {
                                let rest_args = args[i..].to_vec();
                                fe.borrow_mut().set(&rest_name, Value::array(rest_args));
                            } else {
                                let is_rest_param = fd
                                    .params
                                    .get(i + 1)
                                    .map(|p| p.starts_with("..."))
                                    .unwrap_or(false);
                                let arg = if !is_rest_param && i >= rest_idx {
                                    Value::Undefined
                                } else {
                                    args.get(i).cloned().unwrap_or(Value::Undefined)
                                };
                                fe.borrow_mut().set(p, arg);
                            }
                        }
                        if fd.uses_arguments {
                            let args_obj = Value::object(
                                args.iter()
                                    .enumerate()
                                    .map(|(i, v)| (i.to_string(), v.clone()))
                                    .collect(),
                            );
                            args_obj
                                .set_prop("length".to_string(), Value::Number(args.len() as f64))?;
                            fe.borrow_mut().set("arguments", args_obj);
                        }
                        fe
                    }
                };

                let s = std::mem::replace(&mut self.global, fe);
                let r = self.run_program_body(&fd.body);
                self.global = s;
                match r {
                    Err(VmErr::Ret(v)) if is_js_object(&v) => Ok(v),
                    Err(VmErr::Ret(_)) => Ok(inst),
                    _ => Ok(inst),
                }
            }
            _ => {
                let type_name = match f {
                    Value::String(_) => "string",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    Value::Null => "null",
                    Value::Undefined => "undefined",
                    Value::Array(_) => "array",
                    Value::Object { .. } => "object",
                    _ => "unknown",
                };
                vm_err(format!("TypeError: {} is not a constructor", type_name))
            }
        }
    }
}

/// Stack size for a generator coroutine.
///
/// Matched to the main thread's typical 8MB: `MAX_CALL_DEPTH` is calibrated
/// against that, and a smaller stack would overflow before the guest-visible
/// recursion limit could turn it into a catchable `RangeError`. The stack is
/// allocated with a guard page, so an overflow faults rather than corrupting
/// neighbouring memory.
#[cfg(stackful_coroutines)]
const GENERATOR_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Build the coroutine that runs a generator body.
///
/// The body executes on its own stack but on the *calling thread*, switching
/// back to the caller at each `yield`. Returns `None` if the stack could not
/// be allocated, which the caller reports as an immediately-completed
/// generator rather than a crash.
#[cfg(stackful_coroutines)]
fn make_generator_coroutine(
    body: Rc<Vec<Statement>>,
    closure: Option<super::Env>,
    params: Rc<Vec<Rc<str>>>,
    args: Vec<Value>,
    builtins_env: Option<super::Env>,
    gen_depth: u32,
    realm: super::Realm,
) -> Option<crate::value::GenCoroutine> {
    use corosensei::Coroutine;
    use corosensei::stack::DefaultStack;

    let stack = DefaultStack::new(GENERATOR_STACK_SIZE).ok()?;

    // The first `next()` only starts the body; JS discards its argument, since
    // there is no `yield` expression yet for it to become the value of.
    Some(Coroutine::with_stack(
        stack,
        move |yielder, _first_resume| {
            // A fresh interpreter for the body, chained to the builtins so the
            // standard library is reachable, and to the defining scope so closures
            // resolve as they would at the definition site.
            let inherited_global = closure.as_ref().and_then(super::Environment::find_global);
            let mut interp = if let Some(builtins) = builtins_env {
                let mut i = Interpreter::new();
                i.global = Rc::new(RefCell::new(Environment::child(builtins)));
                i
            } else {
                Interpreter::with_builtins()
            };
            if let Some(global) = inherited_global {
                interp.persistent_global = global;
            }
            // One event loop and one module registry across every stack.
            realm.install(&mut interp);
            // Carried so recursion *through* generators stays bounded: each
            // body runs on a fresh interpreter whose call stack starts empty,
            // so `MAX_CALL_DEPTH` alone never sees it.
            interp.gen_depth = gen_depth;

            // SAFETY: `yielder` is borrowed from this coroutine's own stack frame
            // and stays alive for the whole closure. `interp` is created here and
            // dropped when this closure returns or unwinds, so the handle cannot
            // outlive its referent, and `GenYielder` is `!Send`, so it cannot
            // leave this thread. See `crate::value::GenYielder`.
            interp.gen_yielder = Some(unsafe { crate::value::GenYielder::new(yielder) });

            // Bind parameters in a child of the defining scope.
            let parent_env = closure.unwrap_or_else(|| interp.global.clone());
            let fe = Rc::new(RefCell::new(Environment::child(parent_env)));
            for (i, p) in params.iter().enumerate() {
                let arg = args.get(i).cloned().unwrap_or(Value::Undefined);
                fe.borrow_mut().set(p, arg);
            }
            interp.global = fe;

            match interp.run_program_body(&body) {
                Ok(v) | Err(VmErr::Ret(v)) => GenOutcome::Returned(v),
                Err(VmErr::Throw(v)) => GenOutcome::Threw(v),
                // Abandoned while suspended: the initiating `Drop` consumes
                // this; it is never reported to the driver.
                Err(VmErr::Abandon) => GenOutcome::Abandon,
                Err(VmErr::Msg(m)) => GenOutcome::Failed(m),
                Err(VmErr::RuntimeError(e)) => GenOutcome::Failed(e.message.clone()),
                // A break/continue escaping the generator body is a runtime error.
                Err(e @ (VmErr::Break(_) | VmErr::Continue(_))) => {
                    GenOutcome::Failed(format!("{}", e))
                }
            }
        },
    ))
}

/// `Generator.prototype.next`: resumes the generator (starting it on the first
/// call), and produces a `{ value, done }` result object.
#[cfg_attr(not(stackful_coroutines), expect(unused_variables))]
pub(crate) fn generator_next(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let inner_rc = match &this {
        Value::Generator { inner } => inner.clone(),
        _ => return Ok(iter_result(Value::Undefined, true)),
    };

    // Without stack switching a running body cannot be suspended (see
    // `build.rs` for which targets those are). Instead the body runs once, to
    // completion, on the first `next()`, and its yields are buffered for the
    // remaining calls to drain.
    //
    // That is not full generator semantics, and the difference is observable:
    // the body's side effects all happen at the first `next()` rather than
    // interleaved with the consumer, `next(v)` cannot send a value in (every
    // `yield` evaluates to `undefined`), abandoning a `for…of` early does not
    // stop a body that has already run, and an unbounded generator exhausts
    // the loop budget instead of streaming. It is what this target can do
    // without a resumable evaluator, and it is what the values a finite
    // generator produces need.
    #[cfg(not(stackful_coroutines))]
    {
        {
            let mut inner = inner_rc.borrow_mut();
            if !inner.started {
                inner.started = true;
                let body = inner.body.clone();
                let closure = inner.closure.clone();
                let params = inner.params.clone();
                let args = inner.args.clone();
                drop(inner);
                let (produced, returned) =
                    run_buffered_generator(interp, body, closure, params, args)?;
                let mut inner = inner_rc.borrow_mut();
                inner.buffered = produced;
                inner.return_value = Some(returned);
            }
        }
        let mut inner = inner_rc.borrow_mut();
        match inner.buffered.pop_front() {
            Some(value) => return Ok(iter_result(value, false)),
            None => {
                inner.done = true;
                let returned = inner.return_value.clone().unwrap_or(Value::Undefined);
                return Ok(iter_result(returned, true));
            }
        }
    }

    #[cfg(stackful_coroutines)]
    {
        // The coroutine is moved *out* of the shared cell for the duration of
        // the resume. Holding a `RefCell` borrow across it would panic if the
        // body reached back in via `next()`; taking it instead leaves an
        // observable `None`, which is how re-entrancy is detected below.
        let mut coroutine = {
            let mut inner = inner_rc.borrow_mut();

            if inner.done {
                let rv = inner.return_value.clone().unwrap_or(Value::Undefined);
                return Ok(iter_result(rv, true));
            }

            if !inner.started {
                if interp.gen_depth >= super::MAX_GENERATOR_DEPTH {
                    return Err(crate::value::limit_err(
                        "Maximum generator nesting exceeded",
                    ));
                }
                inner.started = true;
                // The builtins scope is the parent of the driver's global.
                let builtins_env = interp.global.borrow().parent_env();
                inner.coroutine = make_generator_coroutine(
                    inner.body.clone(),
                    inner.closure.clone(),
                    inner.params.clone(),
                    inner.args.clone(),
                    builtins_env,
                    interp.gen_depth + 1,
                    super::Realm::of(interp),
                );
                if inner.coroutine.is_none() {
                    // Stack allocation failed; report an exhausted generator.
                    inner.done = true;
                    return Ok(iter_result(Value::Undefined, true));
                }
            }

            match inner.coroutine.take() {
                Some(coroutine) => coroutine,
                // Absent but not finished: the body called `next()` on itself.
                None => {
                    return vm_err("TypeError: Generator is already running");
                }
            }
        };

        let outcome = coroutine.resume(GenResume::Next(args.first().cloned()));

        let mut inner = inner_rc.borrow_mut();
        match outcome {
            corosensei::CoroutineResult::Yield(value) => {
                inner.coroutine = Some(coroutine);
                Ok(iter_result(value, false))
            }
            corosensei::CoroutineResult::Return(GenOutcome::Returned(value)) => {
                inner.done = true;
                inner.return_value = Some(value.clone());
                Ok(iter_result(value, true))
            }
            corosensei::CoroutineResult::Return(GenOutcome::Threw(value)) => {
                inner.done = true;
                Err(VmErr::Throw(value))
            }
            corosensei::CoroutineResult::Return(GenOutcome::Failed(message)) => {
                inner.done = true;
                Err(VmErr::Msg(message))
            }
            // Unreachable: only an abandon-resume produces this, and the
            // initiating `Drop` consumes it. If one ever escapes, report the
            // generator as finished rather than surfacing internals.
            corosensei::CoroutineResult::Return(GenOutcome::Abandon) => {
                inner.done = true;
                Ok(iter_result(Value::Undefined, true))
            }
        }
    }
}

/// Run a generator body to completion, collecting everything it yields.
///
/// Used only where suspension is unavailable. The body runs on a child
/// interpreter with a *yield sink* installed, which is what `yield` and
/// `yield*` push into on that target.
#[cfg(not(stackful_coroutines))]
fn run_buffered_generator(
    interp: &mut Interpreter,
    body: Rc<Vec<Statement>>,
    closure: Option<super::Env>,
    params: Rc<Vec<Rc<str>>>,
    args: Vec<Value>,
) -> Result<(std::collections::VecDeque<Value>, Value), VmErr> {
    let sink: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let parent_env = closure.unwrap_or_else(|| interp.global.clone());
    let frame = Rc::new(RefCell::new(Environment::child(parent_env)));
    for (index, param) in params.iter().enumerate() {
        let arg = args.get(index).cloned().unwrap_or(Value::Undefined);
        frame.borrow_mut().set(param, arg);
    }

    let saved_scope = std::mem::replace(&mut interp.global, frame);
    let saved_sink = interp.yield_sink.replace(sink.clone());
    let outcome = interp.run_program_body(&body);
    interp.yield_sink = saved_sink;
    interp.global = saved_scope;

    let returned = match outcome {
        Ok(value) | Err(VmErr::Ret(value)) => value,
        Err(error) => return Err(error),
    };
    let produced = Rc::try_unwrap(sink)
        .map(RefCell::into_inner)
        .unwrap_or_else(|shared| shared.borrow().clone());
    Ok((produced.into(), returned))
}

/// `Generator.prototype.throw`: raise a value at the suspension point, so a
/// `try`/`catch` around the `yield` inside the body sees it.
pub(crate) fn generator_throw(
    _interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let thrown = args.into_iter().next().unwrap_or(Value::Undefined);
    let Value::Generator { inner } = &this else {
        return Err(VmErr::Throw(thrown));
    };

    // Nothing is suspended on a target without stack switching, and a
    // generator that has not started or has finished re-throws at the caller.
    #[cfg(stackful_coroutines)]
    {
        let mut coroutine = {
            let mut state = inner.borrow_mut();
            if state.done || !state.started {
                state.done = true;
                return Err(VmErr::Throw(thrown));
            }
            match state.coroutine.take() {
                Some(coroutine) => coroutine,
                None => return vm_err("TypeError: Generator is already running"),
            }
        };
        // Cloned so the defensive `Abandon` arm below can still re-throw
        // the caller's value.
        let outcome = coroutine.resume(GenResume::Throw(thrown.clone()));
        let mut state = inner.borrow_mut();
        match outcome {
            corosensei::CoroutineResult::Yield(value) => {
                // The body caught it and yielded again.
                state.coroutine = Some(coroutine);
                Ok(iter_result(value, false))
            }
            corosensei::CoroutineResult::Return(GenOutcome::Returned(value)) => {
                state.done = true;
                state.return_value = Some(value.clone());
                Ok(iter_result(value, true))
            }
            corosensei::CoroutineResult::Return(GenOutcome::Threw(value)) => {
                state.done = true;
                Err(VmErr::Throw(value))
            }
            corosensei::CoroutineResult::Return(GenOutcome::Failed(message)) => {
                state.done = true;
                Err(VmErr::Msg(message))
            }
            // Unreachable: only an abandon-resume produces this, and the
            // initiating `Drop` consumes it. Re-throw as the caller's value
            // rather than surfacing internals (`thrown` was cloned for the
            // resume above so it is still available here).
            corosensei::CoroutineResult::Return(GenOutcome::Abandon) => {
                state.done = true;
                Err(VmErr::Throw(thrown))
            }
        }
    }
    #[cfg(not(stackful_coroutines))]
    {
        inner.borrow_mut().done = true;
        Err(VmErr::Throw(thrown))
    }
}

/// `Generator.prototype.return`: finish the generator, running any `finally`
/// blocks around the suspension point, and report the given value.
pub(crate) fn generator_return(
    _interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.into_iter().next().unwrap_or(Value::Undefined);
    if let Value::Generator { inner } = &this {
        #[cfg(stackful_coroutines)]
        inner.borrow_mut().close();
        #[cfg(not(stackful_coroutines))]
        {
            let mut state = inner.borrow_mut();
            state.done = true;
            state.buffered.clear();
        }
    }
    Ok(iter_result(value, true))
}

/// Build an iterator result object `{ value, done }`.
pub(crate) fn iter_result(value: Value, done: bool) -> Value {
    Value::object(vec![
        ("value".to_string(), value),
        ("done".to_string(), Value::Bool(done)),
    ])
}
