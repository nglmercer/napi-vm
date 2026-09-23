//! Property resolution: direct lookup, prototype-chain walk, and getter
//! invocation.

use super::Interpreter;
use crate::error::VmErr;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use crate::lang::CompletionKind;
use crate::value::{BoxedPrimitive, Value};

impl Interpreter {
    /// Enumerate properties visible on a simple runtime receiver such as
    /// `store` or `user.profile`. This only reads existing values and walks
    /// their prototype objects; it never evaluates guest source.
    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    pub(crate) fn completion_property_members(
        &self,
        receiver: &str,
    ) -> Vec<(String, CompletionKind)> {
        let Some(value) = self.completion_receiver_value(receiver) else {
            return Vec::new();
        };
        let mut members = Vec::new();
        self.collect_completion_members(&value, &mut members, 0);
        members.sort_by(|a, b| a.0.cmp(&b.0));
        members.dedup_by(|a, b| a.0 == b.0);
        members
    }

    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    fn completion_receiver_value(&self, receiver: &str) -> Option<Value> {
        let mut parts = receiver.split('.');
        let first = parts.next()?;
        if !is_completion_identifier(first) {
            return None;
        }
        let mut value = self.global.borrow().get(first)?;
        for part in parts {
            if !is_completion_identifier(part) {
                return None;
            }
            value = self.prop(&value, &Value::String(part.to_string())).ok()?;
        }
        Some(value)
    }

    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    fn collect_completion_members(
        &self,
        value: &Value,
        members: &mut Vec<(String, CompletionKind)>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        let mut add = |name: &str, kind: CompletionKind| {
            if is_completion_identifier(name) && !name.starts_with("__") {
                members.push((name.to_string(), kind));
            }
        };

        match value {
            Value::Object { props } => {
                for (name, property) in props.borrow().iter() {
                    add(name, completion_kind(property));
                }
                if let Some(proto) = props.proto() {
                    self.collect_completion_members(&proto, members, depth + 1);
                }
            }
            Value::Array(_) => {
                for name in
                    crate::lang::catalog::prototype_members(crate::lang::catalog::ProtoKind::Array)
                {
                    add(name, CompletionKind::Method);
                }
            }
            Value::String(_) => {
                for name in
                    crate::lang::catalog::prototype_members(crate::lang::catalog::ProtoKind::String)
                {
                    add(name, CompletionKind::Method);
                }
            }
            Value::Number(_) => {
                for name in
                    crate::lang::catalog::prototype_members(crate::lang::catalog::ProtoKind::Number)
                {
                    add(name, CompletionKind::Method);
                }
            }
            Value::Promise { .. } => {
                for name in crate::lang::catalog::prototype_members(
                    crate::lang::catalog::ProtoKind::Promise,
                ) {
                    add(name, CompletionKind::Method);
                }
            }
            Value::Class(class) => {
                for (name, property) in class.statics.borrow().iter() {
                    add(name, completion_kind(property));
                }
                self.collect_completion_members(&class.prototype, members, depth + 1);
            }
            Value::GlobalObject => {
                for name in self.global.borrow().all_keys() {
                    add(&name, CompletionKind::Global);
                }
            }
            // A proxy completes as what it stands in for.
            Value::Proxy(proxy) => {
                let target = proxy.target.clone();
                self.collect_completion_members(&target, members, depth + 1);
            }
            // Everything else has a fixed member set the catalog already
            // describes, or none at all.
            _ => {}
        }
    }

    /// Stringify a value, honouring a `toString` method the guest defined.
    ///
    /// Not the same as [`Interpreter::vs`], which cannot call guest code: `vs`
    /// runs from `&self` positions, including inside the fused
    /// read-modify-write that holds the scope mutably borrowed, so invoking a
    /// method there would re-enter the interpreter mid-borrow. This is for the
    /// call sites that are safely `&mut` — `String(x)`, template literals and
    /// `console.*` — where a custom `toString` is what a caller expects.
    pub(crate) fn display_string(&mut self, value: &Value) -> Result<String, VmErr> {
        let has_custom = matches!(value, Value::Object { .. } | Value::Error(_));
        if has_custom {
            let method = self.member(value, "toString")?;
            if matches!(
                method,
                Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
            ) {
                let rendered = self.call_this(&method, value.clone(), vec![])?;
                // A primitive return is used, coerced if it is not already a
                // string. Only an object return falls through, the way the
                // specification moves on to the next hint.
                match &rendered {
                    Value::String(text) => return Ok(text.clone()),
                    Value::Object { .. } | Value::Array(_) | Value::Error(_) => {}
                    other => return self.vs(other),
                }
            }
        }
        self.vs(value)
    }

    /// Apply the ECMAScript abstract ToNumber operation to a Node-API value.
    /// Unlike `Value::to_number`, this can call guest conversion methods and
    /// rejects Symbols and BigInts instead of silently manufacturing a number.
    pub(crate) fn napi_to_number(&mut self, value: &Value) -> Result<f64, VmErr> {
        let primitive = self.coerce_object_to_primitive(value, "number")?;
        match &primitive {
            Value::Undefined => Ok(f64::NAN),
            Value::Null => Ok(0.0),
            Value::Bool(value) => Ok(if *value { 1.0 } else { 0.0 }),
            Value::Number(value) => Ok(*value),
            Value::String(value) => Ok(ecmascript_string_to_number(value)),
            Value::Symbol(_) => Err(VmErr::Msg(
                "TypeError: Cannot convert a Symbol value to a number".into(),
            )),
            Value::BigInt(_) => Err(VmErr::Msg(
                "TypeError: Cannot convert a BigInt value to a number".into(),
            )),
            _ => Err(VmErr::Msg(
                "TypeError: Cannot convert object to primitive value".into(),
            )),
        }
    }

    /// Apply ECMAScript ToString for Node-API. This deliberately differs from
    /// the `String(Symbol())` function special case: abstract ToString throws
    /// for Symbols, as does `napi_coerce_to_string`.
    pub(crate) fn napi_to_string(&mut self, value: &Value) -> Result<String, VmErr> {
        let primitive = self.coerce_object_to_primitive(value, "string")?;
        match &primitive {
            Value::Undefined => Ok("undefined".into()),
            Value::Null => Ok("null".into()),
            Value::Bool(value) => Ok(if *value { "true" } else { "false" }.into()),
            Value::Number(value) => Ok(if *value == 0.0 {
                "0".into()
            } else {
                crate::format::number_string(*value)
            }),
            Value::String(value) => Ok(value.clone()),
            Value::BigInt(value) => Ok(value.to_decimal()),
            Value::Symbol(_) => Err(VmErr::Msg(
                "TypeError: Cannot convert a Symbol value to a string".into(),
            )),
            _ => Err(VmErr::Msg(
                "TypeError: Cannot convert object to primitive value".into(),
            )),
        }
    }

    /// Perform ToPrimitive with the requested hint, including the guest's
    /// `Symbol.toPrimitive` hook and ordinary `valueOf`/`toString` order.
    fn coerce_object_to_primitive(&mut self, value: &Value, hint: &str) -> Result<Value, VmErr> {
        let value = value.deref_binding();
        if is_primitive(&value) {
            return Ok(value);
        }

        let exotic_key = crate::builtins::well_known("toPrimitive")
            .expect("Symbol.toPrimitive is a well-known symbol");
        let exotic = self.get_prop_value(&value, &exotic_key)?;
        if !matches!(exotic, Value::Undefined | Value::Null) {
            if !is_callable(&exotic) {
                return Err(VmErr::Msg(
                    "TypeError: Symbol.toPrimitive must be a function".into(),
                ));
            }
            let result =
                self.call_this(&exotic, value.clone(), vec![Value::String(hint.to_owned())])?;
            if is_primitive(&result) {
                return Ok(result);
            }
            return Err(VmErr::Msg(
                "TypeError: Cannot convert object to primitive value".into(),
            ));
        }

        let order = if hint == "string" {
            ["toString", "valueOf"]
        } else {
            ["valueOf", "toString"]
        };
        for name in order {
            let method = self.member(&value, name)?;
            if is_callable(&method) {
                let result = self.call_this(&method, value.clone(), vec![])?;
                if is_primitive(&result) {
                    return Ok(result);
                }
                continue;
            }

            // Object.prototype.valueOf returns its receiver. The VM does not
            // materialize Object.prototype, so preserve that behavior when
            // the ordinary default prototype supplies the missing method.
            if matches!(method, Value::Undefined)
                && name == "valueOf"
                && has_default_object_prototype(&value)
            {
                continue;
            }

            // Object.prototype.toString is likewise supplied by the runtime
            // for ordinary objects whose prototype chain uses that default.
            if matches!(method, Value::Undefined)
                && name == "toString"
                && has_default_object_prototype(&value)
            {
                return Ok(Value::String(self.vs(&value)?));
            }
        }

        Err(VmErr::Msg(
            "TypeError: Cannot convert object to primitive value".into(),
        ))
    }

    /// Does `+` have to reduce this value to a primitive first?
    ///
    /// Objects and arrays do: `1 + [2]` is `"12"`, because the array becomes
    /// the string `"2"` before the operator sees it. Everything else is
    /// already primitive, and `bin_op` handles it directly.
    pub(crate) fn needs_concat_coercion(value: &Value) -> bool {
        matches!(
            value,
            Value::Object { .. } | Value::Array(_) | Value::Error(_) | Value::Proxy(_)
        )
    }

    /// Reduce an operand of `+` to a primitive, the way `ToPrimitive` with no
    /// hint does: `valueOf` first, then `toString`, taking whichever returns a
    /// primitive.
    ///
    /// The order is what makes `1 + obj` numeric addition when `obj` has a
    /// `valueOf`, and string concatenation when it only has a `toString`.
    ///
    /// `bin_op` cannot do this itself: it runs from `&self` positions, so it
    /// has no way to call guest code. Doing it here, before the operator sees
    /// the values, keeps that restriction.
    pub(crate) fn coerce_for_concat(&mut self, value: &Value) -> Result<Value, VmErr> {
        if !Self::needs_concat_coercion(value) {
            return Ok(value.clone());
        }
        for method in ["valueOf", "toString"] {
            let callable = self.member(value, method)?;
            if !matches!(
                callable,
                Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
            ) {
                continue;
            }
            let produced = self.call_this(&callable, value.clone(), vec![])?;
            if !Self::needs_concat_coercion(&produced) {
                return Ok(produced);
            }
        }
        // Neither yielded a primitive: fall back to the built-in rendering.
        Ok(Value::String(self.vs(value)?))
    }

    /// Read a string-keyed property, running a getter if one is installed.
    pub(crate) fn member(&mut self, o: &Value, key: &str) -> Result<Value, VmErr> {
        self.get_prop_value(o, &Value::String(key.to_string()))
    }

    /// Test whether a property exists, using the proxy `has` trap when one is
    /// present. This is the mutable counterpart to `Value::has_prop`, which
    /// cannot run guest code from its shared-reference call sites.
    #[cfg_attr(
        not(all(
            feature = "node-api-host",
            any(target_os = "linux", target_os = "macos")
        )),
        allow(dead_code)
    )]
    pub(crate) fn has_property(&mut self, object: &Value, key: &Value) -> Result<bool, VmErr> {
        let property = self.property_key(key)?;
        if let Some(proxy) = object.as_proxy() {
            let target = proxy.target.clone();
            if let Some(trap) = self.proxy_trap(&proxy, "has") {
                let handler = proxy.handler.clone();
                let result =
                    self.call_this(&trap, handler, vec![target, Value::String(property)])?;
                return Ok(result.is_truthy());
            }
            return self.has_property(&target, &Value::String(property));
        }
        Ok(object.has_prop(&property))
    }

    /// Every value an iterable produces, as a `Vec`.
    pub(crate) fn iterate(&mut self, source: &Value) -> Result<Vec<Value>, VmErr> {
        match source {
            Value::Array(items) => Ok(items.borrow().clone()),
            _ => self.drain_iterable(source),
        }
    }

    /// Resolve a property value, invoking it if it is a getter.
    pub(crate) fn get_prop_value(&mut self, o: &Value, p: &Value) -> Result<Value, VmErr> {
        // A proxy's `get` trap replaces the read entirely; without one the
        // read falls through to the target.
        if let Some(proxy) = o.as_proxy() {
            let target = proxy.target.clone();
            if let Some(trap) = self.proxy_trap(&proxy, "get") {
                let key = Value::String(self.property_key(p)?);
                let handler = proxy.handler.clone();
                return self.call_this(&trap, handler, vec![target, key, o.clone()]);
            }
            return self.get_prop_value(&target, p);
        }
        // Reading a property of `null` or `undefined` is a `TypeError`, not
        // `undefined`. Silently answering `undefined` hides the mistake and
        // makes optional chaining pointless — `o?.a` exists precisely because
        // `o.a` throws.
        if matches!(o, Value::Null | Value::Undefined) {
            let key = self.property_key(p)?;
            return Err(VmErr::Msg(format!(
                "TypeError: Cannot read properties of {} (reading '{}')",
                if matches!(o, Value::Null) {
                    "null"
                } else {
                    "undefined"
                },
                key
            )));
        }
        let v = self.prop(o, p)?;
        let is_getter = match &v {
            Value::Function(f) => f.name.as_ref().is_some_and(|n| n.starts_with("get ")),
            // A native accessor — `Map.prototype.size` — is recognized the
            // same way, by the `get ` prefix on its name.
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.starts_with("get ")
            }
            _ => false,
        };
        let is_setter_only = match &v {
            Value::Function(f) => f.name.as_ref().is_some_and(|n| n.starts_with("set ")),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                name.starts_with("set ")
            }
            _ => false,
        };
        if is_getter {
            return self.call_this(&v, o.clone(), vec![]);
        }
        if is_setter_only {
            return Ok(Value::Undefined);
        }
        Ok(v)
    }

    /// Read a property, resolving a live module binding to the value it names.
    pub(crate) fn prop(&self, o: &Value, p: &Value) -> Result<Value, VmErr> {
        Ok(self.prop_raw(o, p)?.deref_binding())
    }

    fn prop_raw(&self, o: &Value, p: &Value) -> Result<Value, VmErr> {
        match (o, p) {
            // `window.x` / `globalThis.x` / `self.x` read a real global.
            (Value::GlobalObject, Value::String(k)) => Ok(self
                .persistent_global
                .borrow()
                .get(k)
                .unwrap_or(Value::Undefined)),
            (Value::Object { props }, Value::String(k)) => {
                if let Some(value) = lookup_chain_found(o, k)? {
                    return Ok(value);
                }
                let boxed = props.meta.borrow().boxed_primitive.clone();
                Ok(boxed
                    .map(|primitive| boxed_primitive_property(&primitive, k))
                    .unwrap_or(Value::Undefined))
            }
            (Value::Object { props }, Value::Number(index)) => {
                let key = crate::format::number_string(*index);
                if let Some(value) = lookup_chain_found(o, &key)? {
                    return Ok(value);
                }
                let boxed = props.meta.borrow().boxed_primitive.clone();
                Ok(boxed
                    .map(|primitive| boxed_primitive_property(&primitive, &key))
                    .unwrap_or(Value::Undefined))
            }
            (Value::Array(items), Value::Number(i)) => {
                let items = items.borrow();
                if !i.is_finite() || *i < 0.0 || i.fract() != 0.0 {
                    Ok(Value::Undefined)
                } else {
                    let idx = *i as usize;
                    if idx < items.len() {
                        Ok(items[idx].clone())
                    } else {
                        Ok(Value::Undefined)
                    }
                }
            }
            (Value::Array(items), Value::String(k)) => {
                if k == "length" {
                    Ok(Value::Number(items.borrow().len() as f64))
                } else if k == "__symbol_iterator__" {
                    Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: array_iter,
                    })
                } else if let Some(idx) = crate::value::array_index(k) {
                    let items = items.borrow();
                    if idx < items.len() {
                        Ok(items[idx].clone())
                    } else {
                        Ok(Value::Undefined)
                    }
                } else if let Some(m) = crate::builtins::array_method(k) {
                    Ok(m)
                } else {
                    // Arrays can also carry named properties — a tagged
                    // template's `strings.raw` is the built-in example.
                    Ok(items.named_prop(k).unwrap_or(Value::Undefined))
                }
            }
            (Value::String(s), Value::String(k)) => {
                if k == "length" {
                    Ok(Value::Number(s.chars().count() as f64))
                } else if k == "__symbol_iterator__" {
                    Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: string_iter,
                    })
                } else if let Ok(idx) = k.parse::<usize>() {
                    Ok(s.chars()
                        .nth(idx)
                        .map(|c| Value::String(c.to_string()))
                        .unwrap_or(Value::Undefined))
                } else if let Some(m) = crate::builtins::string_method(k) {
                    Ok(m)
                } else {
                    Ok(Value::Undefined)
                }
            }
            (Value::Number(_), Value::String(k)) => {
                if let Some(m) = crate::builtins::number_method(k) {
                    Ok(m)
                } else {
                    Ok(Value::Undefined)
                }
            }
            (Value::Promise { .. }, Value::String(k)) => {
                Ok(crate::builtins::promise_method(k).unwrap_or(Value::Undefined))
            }
            (Value::String(s), Value::Number(i)) => {
                let idx = *i as usize;
                Ok(s.chars()
                    .nth(idx)
                    .map(|c| Value::String(c.to_string()))
                    .unwrap_or(Value::Undefined))
            }
            (Value::Class(c), Value::String(k)) => lookup_chain(
                &Value::Object {
                    props: c.statics.clone(),
                },
                k,
            ),
            (Value::Class(c), Value::Symbol(symbol)) => lookup_chain(
                &Value::Object {
                    props: c.statics.clone(),
                },
                &crate::interpreter::symbol_slot_key(symbol),
            ),
            (Value::Function(function), Value::String(k)) => {
                if k == "prototype" {
                    return Ok(function.prototype_value(o));
                }
                function.ensure_name_length_properties();
                if let Some(value) = lookup_chain_found(
                    &Value::Object {
                        props: function.properties.clone(),
                    },
                    k,
                )? {
                    return Ok(value);
                }
                Ok(match k.as_str() {
                    // These values are inherited from Function.prototype
                    // when a callable's configurable own property is deleted.
                    "name" => Value::String(String::new()),
                    "length" => Value::Number(0.0),
                    _ => crate::builtins::function_method(k).unwrap_or(Value::Undefined),
                })
            }
            (Value::Function(function), Value::Symbol(symbol)) => lookup_chain(
                &Value::Object {
                    props: function.properties.clone(),
                },
                &super::symbol_slot_key(symbol),
            ),
            (Value::Generator { .. }, Value::String(k)) => {
                if k == "next" {
                    Ok(Value::NativeFunction {
                        name: "next".into(),
                        callable: super::call::generator_next,
                    })
                } else if k == "throw" {
                    Ok(Value::NativeFunction {
                        name: "throw".into(),
                        callable: super::call::generator_throw,
                    })
                } else if k == "return" {
                    Ok(Value::NativeFunction {
                        name: "return".into(),
                        callable: super::call::generator_return,
                    })
                } else if k == "__symbol_iterator__" {
                    // Generators are their own iterators: [Symbol.iterator]()
                    // returns `this`.
                    Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: generator_iter_self,
                    })
                } else {
                    Ok(Value::Undefined)
                }
            }
            (Value::StringIterator { .. }, Value::String(k)) if k == "next" => {
                Ok(Value::NativeFunction {
                    name: "next".into(),
                    callable: string_iter_next,
                })
            }
            (Value::NativeFunction { name, .. }, Value::String(k)) => {
                // Well-known symbols and static methods on `Symbol`. A native
                // function cannot carry properties, so they are resolved here.
                if name.as_ref() == "Symbol" {
                    if let Some(symbol) = crate::builtins::well_known(k) {
                        return Ok(symbol);
                    }
                    match k.as_str() {
                        "for" => Ok(Value::NativeFunction {
                            name: "for".into(),
                            callable: crate::builtins::symbol_for,
                        }),
                        "keyFor" => Ok(Value::NativeFunction {
                            name: "keyFor".into(),
                            callable: crate::builtins::symbol_key_for,
                        }),
                        _ => Ok(Value::Undefined),
                    }
                } else {
                    Ok(crate::builtins::function_method(k).unwrap_or(Value::Undefined))
                }
            }

            // A typed array resolves an index to an element and anything
            // else to a member. `note_method` records which name a delegating
            // method was reached under, since a native function is a bare
            // pointer that cannot carry it.
            (Value::TypedArray(view), Value::Number(i)) => {
                if !i.is_finite() || *i < 0.0 || i.fract() != 0.0 {
                    return Ok(Value::Undefined);
                }
                Ok(crate::builtins::read_element(view, *i as usize).unwrap_or(Value::Undefined))
            }
            (Value::TypedArray(view), Value::String(k)) => {
                if let Ok(index) = k.parse::<usize>() {
                    return Ok(
                        crate::builtins::read_element(view, index).unwrap_or(Value::Undefined)
                    );
                }
                crate::builtins::note_method(k);
                Ok(crate::builtins::typed_member(view, k).unwrap_or(Value::Undefined))
            }
            (Value::TypedArray(view), Value::Symbol(_))
                if crate::builtins::is_iterator_symbol(p) =>
            {
                Ok(
                    crate::builtins::typed_member(view, crate::interpreter::SYMBOL_ITERATOR_SLOT)
                        .unwrap_or(Value::Undefined),
                )
            }
            (Value::ArrayBuffer(bytes), Value::String(k)) => {
                Ok(crate::builtins::array_buffer_member(bytes, k).unwrap_or(Value::Undefined))
            }
            (Value::SharedArrayBuffer(bytes), Value::String(k)) => {
                Ok(crate::builtins::shared_array_buffer_member(bytes, k)
                    .unwrap_or(Value::Undefined))
            }
            (Value::DataView(view), Value::String(k)) => {
                if view.buffer.is_detached() && matches!(k.as_str(), "byteLength" | "byteOffset") {
                    return Err(VmErr::Msg(
                        "TypeError: Cannot access a DataView backed by a detached ArrayBuffer"
                            .to_string(),
                    ));
                }
                crate::builtins::note_method(k);
                Ok(crate::builtins::data_view_member(view, k).unwrap_or(Value::Undefined))
            }
            (Value::Date(_), Value::String(k)) => {
                Ok(crate::builtins::date_member(k).unwrap_or(Value::Undefined))
            }
            (Value::BigInt(_), Value::String(k)) => {
                Ok(crate::builtins::bigint_method(k).unwrap_or(Value::Undefined))
            }
            (Value::RegExp(data), Value::String(k)) => {
                Ok(crate::builtins::regexp_member(data, k).unwrap_or(Value::Undefined))
            }
            (Value::Symbol(symbol), Value::String(k)) => match k.as_str() {
                "description" => Ok(symbol
                    .description
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Undefined)),
                other => Ok(crate::builtins::symbol_method(other).unwrap_or(Value::Undefined)),
            },
            (Value::HostFunction { name, .. }, Value::String(k)) => {
                if k == "name" {
                    Ok(Value::String(name.to_string()))
                } else {
                    Ok(crate::builtins::function_method(k).unwrap_or(Value::Undefined))
                }
            }
            // Symbol-keyed property access: `arr[Symbol.iterator]`,
            // `str[Symbol.iterator]`, `gen[Symbol.iterator]`.
            (Value::Array(_), Value::Symbol(_)) if crate::builtins::is_iterator_symbol(p) => {
                Ok(Value::NativeFunction {
                    name: "[Symbol.iterator]".into(),
                    callable: array_iter,
                })
            }
            (Value::String(_), Value::Symbol(_)) if crate::builtins::is_iterator_symbol(p) => {
                Ok(Value::NativeFunction {
                    name: "[Symbol.iterator]".into(),
                    callable: string_iter,
                })
            }
            (Value::Generator { .. }, Value::Symbol(_))
                if crate::builtins::is_iterator_symbol(p) =>
            {
                Ok(Value::NativeFunction {
                    name: "[Symbol.iterator]".into(),
                    callable: generator_iter_self,
                })
            }
            (Value::StringIterator { .. }, Value::Symbol(_))
                if crate::builtins::is_iterator_symbol(p) =>
            {
                Ok(Value::NativeFunction {
                    name: "[Symbol.iterator]".into(),
                    callable: string_iter_self,
                })
            }
            // Object symbol-keyed lookup: `obj[Symbol.iterator]` resolves the
            // internal `__symbol_iterator__` property.
            (Value::Object { props }, Value::Symbol(symbol)) => {
                if let Some(value) = lookup_chain_found(o, &super::symbol_slot_key(symbol))? {
                    return Ok(value);
                }
                if crate::builtins::is_iterator_symbol(p)
                    && matches!(
                        props.meta.borrow().boxed_primitive.as_ref(),
                        Some(BoxedPrimitive::String(_))
                    )
                {
                    Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: string_iter,
                    })
                } else {
                    Ok(Value::Undefined)
                }
            }
            // Internal errors surface to guest `catch` blocks as error objects
            // with readable `name`/`message` properties.
            (Value::Error(e), Value::String(k)) => match k.as_str() {
                "message" => Ok(Value::String(e.message.clone())),
                "name" => Ok(Value::String(e.name.clone())),
                "stack" => Ok(Value::String(e.stack.clone())),
                "code" => Ok(e.code.clone().map_or(Value::Undefined, Value::String)),
                "toString" => Ok(crate::builtins::error_to_string()),
                _ => Ok(Value::Undefined),
            },
            _ => Ok(Value::Undefined),
        }
    }
}

/// Walk an object's prototype chain looking for `key`, bounded by
/// [`MAX_PROTOTYPE_DEPTH`](crate::value::MAX_PROTOTYPE_DEPTH) so a guest-built
/// cycle spends bounded time instead of hanging.
fn lookup_chain(o: &Value, key: &str) -> Result<Value, VmErr> {
    Ok(lookup_chain_found(o, key)?.unwrap_or(Value::Undefined))
}

/// Look up an object property without conflating a missing property with an
/// own or inherited property whose value is `undefined`.
fn lookup_chain_found(o: &Value, key: &str) -> Result<Option<Value>, VmErr> {
    let mut current = o.clone();
    for _ in 0..=crate::value::MAX_PROTOTYPE_DEPTH {
        let props = match &current {
            Value::Object { props } => props,
            Value::Class(class) => &class.statics,
            Value::Function(function) => &function.properties,
            _ => return Ok(None),
        };
        if let Some((_, value)) = props.borrow().iter().find(|(xk, _)| xk == key) {
            return Ok(Some(value.clone()));
        }
        let Some(next) = props.proto() else {
            return Ok(None);
        };
        current = next.as_ref().clone();
    }
    Err(crate::value::limit_err(
        "Maximum prototype chain depth exceeded",
    ))
}

fn boxed_primitive_value(primitive: &BoxedPrimitive) -> Value {
    match primitive {
        BoxedPrimitive::Bool(value) => Value::Bool(*value),
        BoxedPrimitive::Number(value) => Value::Number(*value),
        BoxedPrimitive::String(value) => Value::String(value.clone()),
        BoxedPrimitive::Symbol(value) => Value::Symbol(value.clone()),
        BoxedPrimitive::BigInt(value) => Value::BigInt(value.clone()),
    }
}

fn boxed_primitive_property(primitive: &BoxedPrimitive, key: &str) -> Value {
    match primitive {
        BoxedPrimitive::String(value) => {
            if key == "length" {
                return Value::Number(value.chars().count() as f64);
            }
            if let Some(index) = crate::value::array_index(key) {
                return value
                    .chars()
                    .nth(index)
                    .map(|character| Value::String(character.to_string()))
                    .unwrap_or(Value::Undefined);
            }
            if key == "toString" || key == "valueOf" {
                return Value::NativeFunction {
                    name: key.into(),
                    callable: boxed_primitive_value_of,
                };
            }
            crate::builtins::string_method(key).unwrap_or(Value::Undefined)
        }
        BoxedPrimitive::Number(_) => {
            if key == "valueOf" {
                Value::NativeFunction {
                    name: "valueOf".into(),
                    callable: boxed_primitive_value_of,
                }
            } else {
                crate::builtins::number_method(key).unwrap_or(Value::Undefined)
            }
        }
        BoxedPrimitive::Bool(_) => match key {
            "toString" | "valueOf" => Value::NativeFunction {
                name: key.into(),
                callable: if key == "toString" {
                    boxed_primitive_to_string
                } else {
                    boxed_primitive_value_of
                },
            },
            _ => Value::Undefined,
        },
        BoxedPrimitive::Symbol(_) => {
            if key == "description"
                && let BoxedPrimitive::Symbol(symbol) = primitive
            {
                return symbol
                    .description
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Undefined);
            }
            if key == "valueOf" {
                Value::NativeFunction {
                    name: "valueOf".into(),
                    callable: boxed_primitive_value_of,
                }
            } else if key == "toString" {
                Value::NativeFunction {
                    name: "toString".into(),
                    callable: boxed_primitive_to_string,
                }
            } else {
                Value::Undefined
            }
        }
        BoxedPrimitive::BigInt(_) => match key {
            "toString" => Value::NativeFunction {
                name: "toString".into(),
                callable: boxed_primitive_to_string,
            },
            "valueOf" => Value::NativeFunction {
                name: "valueOf".into(),
                callable: boxed_primitive_value_of,
            },
            _ => Value::Undefined,
        },
    }
}

fn boxed_primitive_receiver(this: &Value) -> Result<BoxedPrimitive, VmErr> {
    if let Value::Object { props } = this
        && let Some(primitive) = props.meta.borrow().boxed_primitive.clone()
    {
        return Ok(primitive);
    }
    Err(VmErr::Msg(
        "TypeError: method called on an incompatible object".into(),
    ))
}

fn boxed_primitive_value_of(
    _interpreter: &mut Interpreter,
    this: Value,
    _args: Vec<Value>,
) -> Result<Value, VmErr> {
    Ok(boxed_primitive_value(&boxed_primitive_receiver(&this)?))
}

fn boxed_primitive_to_string(
    _interpreter: &mut Interpreter,
    this: Value,
    _args: Vec<Value>,
) -> Result<Value, VmErr> {
    let text = match boxed_primitive_receiver(&this)? {
        BoxedPrimitive::Bool(value) => value.to_string(),
        BoxedPrimitive::Number(value) => crate::format::number_string(value),
        BoxedPrimitive::String(value) => value,
        BoxedPrimitive::Symbol(value) => value.to_display(),
        BoxedPrimitive::BigInt(value) => value.to_decimal(),
    };
    Value::checked_string(text)
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn is_completion_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.chars().enumerate().all(|(index, c)| {
            if index == 0 {
                c.is_ascii_alphabetic() || c == '_' || c == '$'
            } else {
                c.is_ascii_alphanumeric() || c == '_' || c == '$'
            }
        })
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn completion_kind(value: &Value) -> CompletionKind {
    match value {
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => CompletionKind::Method,
        _ => CompletionKind::Property,
    }
}

// --- Iterator protocol native functions -------------------------------------

/// `[Symbol.iterator]()` on a generator returns the generator itself (generators
/// are their own iterators).
fn generator_iter_self(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    Ok(this)
}

/// `[Symbol.iterator]()` on an array returns a new array iterator object with a
/// `next()` method that walks the elements.
fn array_iter(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let items = match &this {
        super::Value::Array(a) => a.borrow().clone(),
        _ => vec![],
    };
    let cursor = Rc::new(RefCell::new(0usize));
    let items_rc = Rc::new(items);

    // Build an iterator object with a `next` method implemented as a closure
    // captured in a NativeFunction. Since NativeFunction takes a plain fn
    // pointer, we store the state in the object's properties and use a
    // stateful approach via a shared counter.
    let cursor_clone = cursor.clone();
    let items_clone = items_rc.clone();

    // We store the iterator state in the object itself and use a native
    // function that reads it back. The trick: store items and cursor index
    // as hidden properties on the iterator object.
    let iter_obj = super::Value::object(vec![
        (
            "__items__".to_string(),
            super::Value::array((*items_rc).clone()),
        ),
        ("__cursor__".to_string(), super::Value::Number(0.0)),
        (
            "next".to_string(),
            super::Value::NativeFunction {
                name: "next".into(),
                callable: array_iter_next,
            },
        ),
        // An iterator is itself iterable, which is what makes
        // `[...map.keys()]` and `for (const k of map.keys())` work.
        (
            crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string(),
            super::Value::NativeFunction {
                name: "[Symbol.iterator]".into(),
                callable: string_iter_self,
            },
        ),
    ]);

    // Suppress unused variable warnings for the closure-based approach we
    // didn't end up using.
    let _ = (cursor_clone, items_clone);

    Ok(iter_obj)
}

/// `next()` for an array iterator: reads `__items__` and `__cursor__` from
/// `this`, advances the cursor, and returns `{value, done}`.
fn array_iter_next(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    let items_prop = this.get_prop("__items__");
    let items = match &items_prop {
        Some(super::Value::Array(a)) => a.borrow().clone(),
        _ => vec![],
    };
    let cursor = match this.get_prop("__cursor__") {
        Some(super::Value::Number(n)) => n as usize,
        _ => 0,
    };

    if cursor < items.len() {
        let val = items[cursor].clone();
        this.set_prop(
            "__cursor__".to_string(),
            super::Value::Number((cursor + 1) as f64),
        )?;
        Ok(super::call::iter_result(val, false))
    } else {
        Ok(super::call::iter_result(super::Value::Undefined, true))
    }
}

/// `[Symbol.iterator]()` on a string returns a character iterator.
fn string_iter(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let source: Rc<str> = match &this {
        super::Value::String(s) => Rc::from(s.clone()),
        _ => Rc::from(""),
    };

    Ok(super::Value::StringIterator {
        inner: Rc::new(RefCell::new(crate::value::StringIteratorData {
            source,
            cursor: 0,
        })),
    })
}

/// `next()` for the lazy string iterator. The cursor is a UTF-8 byte offset,
/// but the yielded value is always one complete Unicode scalar value.
fn string_iter_next(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    let super::Value::StringIterator { inner } = &this else {
        return Ok(super::call::iter_result(super::Value::Undefined, true));
    };
    let mut state = inner.borrow_mut();
    let Some(rest) = state.source.get(state.cursor..) else {
        return Ok(super::call::iter_result(super::Value::Undefined, true));
    };
    let Some(ch) = rest.chars().next() else {
        return Ok(super::call::iter_result(super::Value::Undefined, true));
    };
    state.cursor += ch.len_utf8();
    Ok(super::call::iter_result(
        super::Value::String(ch.to_string()),
        false,
    ))
}

fn string_iter_self(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    Ok(this)
}

fn is_primitive(value: &Value) -> bool {
    matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Symbol(_)
            | Value::BigInt(_)
    )
}

fn is_callable(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    )
}

fn has_default_object_prototype(value: &Value) -> bool {
    match value {
        Value::Object { props } => {
            let meta = props.meta.borrow();
            meta.uses_default_prototype || meta.proto.is_some()
        }
        Value::Proxy(proxy) => has_default_object_prototype(&proxy.target),
        // Built-in object kinds use their own prototype methods where the VM
        // models them; missing Object methods come from Object.prototype.
        _ => true,
    }
}

fn ecmascript_string_to_number(input: &str) -> f64 {
    let input = input.trim_matches(is_ecmascript_whitespace);
    if input.is_empty() {
        return 0.0;
    }

    let has_sign = matches!(input.as_bytes().first(), Some(b'+' | b'-'));
    let (sign, unsigned) = match input.as_bytes()[0] {
        b'+' => (1.0, &input[1..]),
        b'-' => (-1.0, &input[1..]),
        _ => (1.0, input),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }

    // StringNumericLiteral accepts unsigned binary, octal, and hexadecimal
    // forms. A leading sign makes these forms invalid (`Number("-0x1")`).
    if !has_sign && unsigned.len() >= 2 && unsigned.as_bytes()[0] == b'0' {
        let radix = match unsigned.as_bytes()[1] {
            b'x' | b'X' => Some(16),
            b'o' | b'O' => Some(8),
            b'b' | b'B' => Some(2),
            _ => None,
        };
        if let Some(radix) = radix {
            return parse_radix_number(&unsigned[2..], radix).unwrap_or(f64::NAN);
        }
    }

    if !is_decimal_numeric_literal(unsigned) {
        return f64::NAN;
    }
    input.parse::<f64>().unwrap_or(f64::NAN)
}

fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

fn is_decimal_numeric_literal(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        digits += 1;
        index += 1;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            digits += 1;
            index += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}

fn parse_radix_number(input: &str, radix: u32) -> Option<f64> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0.0;
    for character in input.chars() {
        value = value * radix as f64 + character.to_digit(radix)? as f64;
    }
    Some(value)
}
