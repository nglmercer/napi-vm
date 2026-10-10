//! Property resolution: direct lookup, prototype-chain walk, and getter
//! invocation.

use super::Interpreter;
use crate::error::VmErr;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use crate::lang::CompletionKind;
use crate::value::{BoxedPrimitive, FunctionData, Value};
use std::rc::Rc;

impl Interpreter {
    /// ToBigInt is shared by typed elements and atomic operations. Unlike
    /// BigInt(number), it rejects Number primitives after observable coercion.
    pub(crate) fn ecmascript_to_bigint(&mut self, value: &Value) -> Result<Value, VmErr> {
        let primitive = self.coerce_object_to_primitive(value, "number")?;
        let bigint = match &primitive {
            Value::BigInt(value) => return Ok(Value::BigInt(value.clone())),
            Value::Bool(value) => crate::bigint::BigInt::from_i64(i64::from(*value)),
            Value::String(value) => crate::bigint::BigInt::parse(value)
                .map_err(|_| VmErr::Msg("SyntaxError: invalid BigInt value".into()))?,
            _ => {
                return Err(VmErr::Msg(
                    "TypeError: Cannot convert value to a BigInt".into(),
                ));
            }
        };
        Ok(Value::BigInt(Rc::new(bigint)))
    }
    /// ECMAScript GetMethod, shared by iterator protocols and Proxy traps.
    /// Resolve getters before checking callability and treat only nullish
    /// values as absent.
    pub(crate) fn get_method(
        &mut self,
        receiver: &Value,
        key: &Value,
    ) -> Result<Option<Value>, VmErr> {
        let method = self.get_prop_value(receiver, key)?;
        if matches!(method, Value::Undefined | Value::Null) {
            return Ok(None);
        }
        if !super::call::is_callable_value(&method) {
            return Err(VmErr::Msg(
                "TypeError: Property method must be callable".into(),
            ));
        }
        Ok(Some(method))
    }
    /// Primitive references are boxed with the current execution realm's
    /// intrinsic prototype, regardless of where the primitive was obtained.
    fn primitive_reference_prototype(&self, value: &Value) -> Option<Value> {
        let name = match value {
            Value::Bool(_) => "Boolean",
            Value::Number(_) => "Number",
            Value::String(_) => "String",
            Value::BigInt(_) => "BigInt",
            Value::Symbol(_) => "Symbol",
            _ => return None,
        };
        self.persistent_global
            .borrow()
            .intrinsic(name)
            .and_then(|constructor| constructor.get_prop("prototype"))
    }
    pub(crate) fn lookup_binding_in(
        &mut self,
        scope: &super::Env,
        name: &str,
    ) -> Result<super::Lookup, VmErr> {
        let mut frame = Some(scope.clone());
        while let Some(environment) = frame {
            let (local, object, parent, global) = {
                let frame = environment.borrow();
                (
                    frame.own_lexical_binding(name),
                    frame.with_object.clone(),
                    frame.parent_env(),
                    frame.is_global_scope(),
                )
            };
            if global {
                if local {
                    return Ok(environment.borrow().lookup(name));
                }
                let object = Value::RealmGlobal(environment.clone());
                return if self
                    .has_property(&object, &Value::String(crate::JsString::from_key(name)))?
                {
                    self.get_prop_value_str(&object, name)
                        .map(super::Lookup::Value)
                } else {
                    Ok(super::Lookup::Missing)
                };
            }
            if local {
                return Ok(environment.borrow().lookup(name));
            }
            if let Some(object) = object
                && self.with_has_binding(&object, name)?
            {
                return self
                    .get_prop_value_str(&object, name)
                    .map(super::Lookup::Value);
            }
            frame = parent;
        }
        let global = self.realm_global_object();
        if let Some(prototype) = self.prototype_of(&global)
            && prototype.has_prop(name)
        {
            return self
                .get_prop_value_str_with_receiver(&prototype, name, &global)
                .map(super::Lookup::Value);
        }
        Ok(super::Lookup::Missing)
    }

    fn with_has_binding(&mut self, object: &Value, name: &str) -> Result<bool, VmErr> {
        if !self.has_property(object, &Value::String(crate::JsString::from_key(name)))? {
            return Ok(false);
        }
        if let Some(symbol) = crate::builtins::well_known("unscopables") {
            let exclusions = self.get_prop_value(object, &symbol)?;
            if super::call::is_js_object(&exclusions) {
                let excluded = self.get_prop_value_str(&exclusions, name)?;
                if self.truthy(&excluded) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Resolve object-backed bindings without holding an environment borrow
    /// across guest accessors or Proxy operations.
    pub(crate) fn with_binding_object(
        &mut self,
        scope: &super::Env,
        name: &str,
    ) -> Result<Option<Value>, VmErr> {
        let mut frame = Some(scope.clone());
        while let Some(environment) = frame {
            let (local, object, parent, global) = {
                let frame = environment.borrow();
                (
                    frame.own_lexical_binding(name),
                    frame.with_object.clone(),
                    frame.parent_env(),
                    frame.is_global_scope(),
                )
            };
            if global {
                if local {
                    return Ok(None);
                }
                let object = Value::RealmGlobal(environment.clone());
                return self
                    .has_property(&object, &Value::String(crate::JsString::from_key(name)))
                    .map(|has| has.then_some(object));
            }
            if local {
                return Ok(None);
            }
            if let Some(object) = object
                && self.with_has_binding(&object, name)?
            {
                return Ok(Some(object));
            }
            frame = parent;
        }
        Ok(None)
    }

    /// Resolve a deletion reference using the same environment ordering as
    /// reads and writes. Global object bindings use property configurability;
    /// declarative bindings are not deletable. With exclusions apply before
    /// continuing to the outer environment.
    pub(crate) fn delete_binding_in(
        &mut self,
        scope: &super::Env,
        name: &str,
    ) -> Result<Value, VmErr> {
        let mut frame = Some(scope.clone());
        while let Some(environment) = frame {
            let (local, global, object, parent) = {
                let frame = environment.borrow();
                (
                    frame.own_lexical_binding(name),
                    frame.is_global_scope(),
                    frame.with_object.clone(),
                    frame.parent_env(),
                )
            };
            if local {
                return Ok(Value::Bool(false));
            }
            if global {
                let object = Value::RealmGlobal(environment.clone());
                if self.has_property(&object, &Value::String(crate::JsString::from_key(name)))? {
                    let result = self
                        .delete_member(&object, &Value::String(crate::JsString::from_key(name)))?;
                    if matches!(result, Value::Bool(true)) {
                        environment.borrow_mut().remove_global_var_name(name);
                    }
                    return Ok(result);
                }
                return Ok(Value::Bool(true));
            }
            if let Some(object) = object
                && self.with_has_binding(&object, name)?
            {
                return self
                    .delete_member(&object, &Value::String(crate::JsString::from_key(name)));
            }
            frame = parent;
        }
        Ok(Value::Bool(true))
    }

    /// Resolve an object's represented [[Prototype]], including the realm's
    /// default Object.prototype and Function.prototype links that are stored
    /// as defaults rather than copied into every property cell.
    pub(crate) fn prototype_of(&self, object: &Value) -> Option<std::rc::Rc<Value>> {
        if let Some(properties) = object.exotic_properties()
            && !properties.meta.borrow().uses_default_prototype
        {
            return properties.proto();
        }
        if let Some(global) = self.global_scope_of(object) {
            let scope = global.borrow();
            let record = scope.global_object()?;
            let props = record.property_cell()?;
            if !props.meta.borrow().uses_default_prototype {
                return props.proto();
            }
            return scope
                .intrinsic("Object")?
                .get_prop("prototype")
                .map(Rc::new);
        }
        if let Some(prototype) = object.proto_of() {
            return Some(prototype);
        }
        let (builtin, uses_default) = match object {
            Value::Object { props } => {
                let meta = props.meta.borrow();
                let builtin = match meta.boxed_primitive.as_ref() {
                    Some(BoxedPrimitive::Bool(_)) => "Boolean",
                    Some(BoxedPrimitive::Number(_)) => "Number",
                    Some(BoxedPrimitive::String(_)) => "String",
                    Some(BoxedPrimitive::Symbol(_)) => "Symbol",
                    Some(BoxedPrimitive::BigInt(_)) => "BigInt",
                    None => "Object",
                };
                (builtin, meta.uses_default_prototype)
            }
            Value::Array(array) => ("Array", array.meta.borrow().uses_default_prototype),
            Value::Class(class) => (
                "Function",
                class.statics.meta.borrow().uses_default_prototype,
            ),
            Value::Function(function) => (
                "Function",
                function.properties.meta.borrow().uses_default_prototype,
            ),
            Value::Promise(_) => ("Promise", true),
            Value::Date(_) => ("Date", true),
            Value::RegExp(_) => ("RegExp", true),
            Value::ArrayBuffer(_) => ("ArrayBuffer", true),
            Value::SharedArrayBuffer(_) => ("SharedArrayBuffer", true),
            Value::TypedArray(view) if view.is_buffer => ("Buffer", true),
            Value::TypedArray(view) => (view.kind.name(), true),
            Value::DataView(_) => ("DataView", true),
            Value::Error(error) => (error.name.as_str(), true),
            Value::GlobalObject => ("Object", true),
            Value::NativeFunction { .. } | Value::HostFunction { .. } => ("Function", true),
            _ => return None,
        };
        if !uses_default {
            return None;
        }
        let owner =
            super::realm::value_realm(object).unwrap_or_else(|| self.persistent_global.clone());
        let prototype = owner
            .borrow()
            .intrinsic(builtin)
            .and_then(|constructor| constructor.get_prop("prototype"))?;
        if crate::interpreter::strict_equals(object, &prototype) {
            return if builtin == "Object" {
                None
            } else {
                owner
                    .borrow()
                    .intrinsic("Object")
                    .and_then(|constructor| constructor.get_prop("prototype"))
                    .map(Rc::new)
            };
        }
        Some(std::rc::Rc::new(prototype))
    }

    /// Enumerate properties visible on a simple runtime receiver such as
    /// `store` or `user.profile`. This only reads existing values and walks
    /// their prototype objects; it never evaluates guest source.
    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    pub fn completion_property_members(&self, receiver: &str) -> Vec<(String, CompletionKind)> {
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
            value = self
                .prop(&value, &Value::String((part.to_string()).into()))
                .ok()?;
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
                let target = proxy.target_for_inspection();
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
    #[doc(hidden)]
    pub fn display_string(&mut self, value: &Value) -> Result<crate::JsString, VmErr> {
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
                    other => return self.to_js_string(other),
                }
            }
        }
        self.to_js_string(value)
    }

    /// Apply ECMAScript's abstract ToNumber operation. Unlike `Value::to_number`,
    /// this can call guest conversion methods and rejects Symbols and BigInts
    /// instead of silently manufacturing a number.
    pub(crate) fn ecmascript_to_number(&mut self, value: &Value) -> Result<f64, VmErr> {
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

    /// Apply ECMAScript ToString, invoking guest conversion hooks on the VM
    /// owner thread. This deliberately differs from
    /// the `String(Symbol())` function special case: abstract ToString throws
    /// for Symbols, as does `napi_coerce_to_string`.
    pub fn ecmascript_to_string(&mut self, value: &Value) -> Result<crate::JsString, VmErr> {
        let primitive = self.coerce_object_to_primitive(value, "string")?;
        match primitive {
            Value::Symbol(_) => Err(VmErr::Msg(
                "TypeError: Cannot convert a Symbol value to a string".into(),
            )),
            ref value if is_primitive(value) => self.to_js_string(value),
            _ => Err(VmErr::Msg(
                "TypeError: Cannot convert object to primitive value".into(),
            )),
        }
    }

    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    pub(crate) fn napi_to_string(&mut self, value: &Value) -> Result<crate::JsString, VmErr> {
        self.ecmascript_to_string(value)
    }

    /// Perform ToPrimitive with the requested hint, including the guest's
    /// `Symbol.toPrimitive` hook and ordinary `valueOf`/`toString` order.
    pub(crate) fn coerce_object_to_primitive(
        &mut self,
        value: &Value,
        hint: &str,
    ) -> Result<Value, VmErr> {
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
            let result = self.call_this(
                &exotic,
                value.clone(),
                vec![Value::String((hint.to_owned()).into())],
            )?;
            if is_primitive(&result) {
                return Ok(result);
            }
            return Err(VmErr::Msg(
                "TypeError: Cannot convert object to primitive value".into(),
            ));
        }

        let order = if hint == "string" || (hint == "default" && matches!(value, Value::Date(_))) {
            ["toString", "valueOf"]
        } else {
            ["valueOf", "toString"]
        };
        let materialized_object_prototype = self
            .persistent_global
            .borrow()
            .intrinsic("Object")
            .and_then(|constructor| constructor.get_prop("prototype"))
            .is_some();
        for name in order {
            let method = self.member(&value, name)?;
            if is_callable(&method) {
                let result = self.call_this(&method, value.clone(), vec![])?;
                if is_primitive(&result) {
                    return Ok(result);
                }
                continue;
            }

            // Bare interpreters can omit the intrinsic bootstrap. Preserve
            // default valueOf behavior only in that configuration; a guest
            // shadowing a materialized method with undefined must be honored.
            if matches!(method, Value::Undefined)
                && !materialized_object_prototype
                && name == "valueOf"
                && has_default_object_prototype(&value)
            {
                continue;
            }

            // Supply the bare interpreter's default toString on the same
            // terms, without replacing explicitly shadowed guest methods.
            if matches!(method, Value::Undefined)
                && !materialized_object_prototype
                && name == "toString"
                && has_default_object_prototype(&value)
            {
                return Ok(Value::String((self.vs(&value)?).into()));
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
        !is_primitive(value)
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
        self.coerce_object_to_primitive(value, "default")
    }

    /// Render a template literal from its cooked chunks and evaluated
    /// interpolation values. Shared by the AST evaluator and the bytecode VM
    /// so both tiers agree (including the length-cap error).
    pub(crate) fn render_template(
        &mut self,
        quasis: &[crate::JsString],
        values: &[Value],
    ) -> Result<Value, VmErr> {
        let mut result = crate::JsString::default();
        for (i, q) in quasis.iter().enumerate() {
            if result.len().saturating_add(q.len()) > crate::value::MAX_STRING_LEN {
                return Err(crate::value::limit_err("Maximum string length exceeded"));
            }
            result.push_str(q);
            if i < values.len() {
                let rendered = self.display_string(&values[i])?;
                if result.len().saturating_add(rendered.len()) > crate::value::MAX_STRING_LEN {
                    return Err(crate::value::limit_err("Maximum string length exceeded"));
                }
                result.push_str(&rendered);
            }
        }
        Value::checked_string(result)
    }

    /// Read a string-keyed property, running a getter if one is installed.
    #[doc(hidden)]
    pub fn member(&mut self, o: &Value, key: &str) -> Result<Value, VmErr> {
        let value = self.get_prop_value_str(o, key)?;
        if matches!(value, Value::Uninitialized) {
            return Err(VmErr::Msg(format!(
                "ReferenceError: Cannot access '{key}' before initialization"
            )));
        }
        Ok(value)
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
        let trap_key = self.to_property_key(key)?;
        let property = self.property_key(&trap_key)?;
        let mut current = object.clone();
        for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
            if let Value::Proxy(proxy) = &current {
                let (target, handler) = proxy.snapshot()?;
                if let Some(trap) = self.proxy_trap(&handler, "has")? {
                    let accepted = self
                        .call_this(&trap, handler, vec![target.clone(), trap_key.clone()])?
                        .is_truthy();
                    if !accepted {
                        let descriptor = crate::builtins::object::descriptor_for_key_in(
                            self, &target, &trap_key,
                        )?;
                        if !matches!(descriptor, Value::Undefined)
                            && (!descriptor
                                .get_prop("configurable")
                                .is_some_and(|value| value.is_truthy())
                                || !self.is_extensible(&target)?)
                        {
                            return Err(VmErr::Msg(
                                "TypeError: Proxy has trap cannot hide a protected property".into(),
                            ));
                        }
                    }
                    return Ok(accepted);
                }
                current = target;
                continue;
            }
            if let Value::TypedArray(view) = &current
                && let Some(index) = crate::builtins::canonical_numeric_index(&property)
            {
                return Ok(crate::builtins::valid_integer_index(view, index));
            }
            // Named own slots already establish presence; avoid materializing
            // a descriptor on ordinary [[HasProperty]] hits. Virtual/exotic
            // properties still use their shared descriptor operation below.
            if current
                .property_cell()
                .is_some_and(|cell| cell.own_index(&property).is_some())
            {
                return Ok(true);
            }
            let descriptor =
                crate::builtins::object::descriptor_for_key_in(self, &current, &trap_key)?;
            if !matches!(descriptor, Value::Undefined) {
                return Ok(true);
            }
            current = self.get_prototype_of(&current)?;
            if matches!(current, Value::Null) {
                return Ok(false);
            }
        }
        Err(crate::value::limit_err("Maximum prototype depth exceeded"))
    }

    /// Every value an iterable produces, as a `Vec`.
    #[doc(hidden)]
    pub fn iterate(&mut self, source: &Value) -> Result<Vec<Value>, VmErr> {
        match source {
            Value::Array(items) => Ok(items.borrow().clone()),
            _ => self.drain_iterable(source),
        }
    }

    /// Resolve a property value, invoking it if it is a getter.
    pub(crate) fn get_prop_value(&mut self, o: &Value, p: &Value) -> Result<Value, VmErr> {
        self.get_prop_value_with_receiver(o, p, o)
    }

    fn with_property_get<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, VmErr>,
    ) -> Result<T, VmErr> {
        self.check_execution_interrupt()?;
        if self.property_get_depth >= crate::value::MAX_PROTOTYPE_DEPTH {
            return Err(crate::value::limit_err(
                "Maximum property operation depth exceeded",
            ));
        }
        self.property_get_depth += 1;
        let result = stacker::maybe_grow(1024 * 1024, 8 * 1024 * 1024, || operation(self));
        self.property_get_depth -= 1;
        result
    }

    pub(crate) fn get_prop_value_with_receiver(
        &mut self,
        o: &Value,
        p: &Value,
        receiver: &Value,
    ) -> Result<Value, VmErr> {
        self.with_property_get(|vm| vm.get_prop_value_with_receiver_inner(o, p, receiver))
    }

    fn get_prop_value_with_receiver_inner(
        &mut self,
        o: &Value,
        p: &Value,
        receiver: &Value,
    ) -> Result<Value, VmErr> {
        if let Value::RealmGlobal(global) = o {
            return self.with_global_storage(global.clone(), |vm| {
                vm.get_prop_value_with_receiver(&Value::GlobalObject, p, receiver)
            });
        }
        if matches!(o, Value::GlobalObject) {
            let record = self
                .persistent_global
                .borrow()
                .global_object()
                .expect("global object record");
            return self.get_prop_value_with_receiver(&record, p, receiver);
        }
        // String keys take the borrowed-key path below, which never allocates
        // a key `Value`. Only symbols, numbers, and exotic keys stay here.
        if let Value::String(key) = p {
            return self.get_prop_value_str_with_receiver(o, &key.to_key(), receiver);
        }
        if !matches!(p, Value::Symbol(_)) {
            let key = self.property_key(p)?;
            return self.get_prop_value_str_with_receiver(o, &key, receiver);
        }
        // A proxy's `get` trap replaces the read entirely; without one the
        // read falls through to the target.
        if let Some(proxy) = o.as_proxy() {
            let (target, handler) = proxy.snapshot()?;
            if let Some(trap) = self.proxy_trap(&handler, "get")? {
                let key = self.to_property_key(p)?;

                let result = self.call_this(
                    &trap,
                    handler,
                    vec![target.clone(), key.clone(), receiver.clone()],
                )?;
                return self.validate_proxy_get(&target, &key, result);
            }
            return self.get_prop_value_with_receiver(&target, p, receiver);
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
        if let Some(props) = match o {
            Value::Object { props } => Some(props.clone()),
            _ if !self.seq(o, receiver) => o.exotic_properties(),
            _ => None,
        } {
            let key = self.to_property_key(p)?;
            let slot = self.property_key(&key)?;
            let inherited = {
                let own = props.borrow().iter().any(|(name, _)| name == &slot);
                if own { None } else { props.proto() }
            };
            if let Some(prototype) = inherited
                && matches!(
                    crate::builtins::object::descriptor_for_key_in(self, o, &key)?,
                    Value::Undefined
                )
            {
                return self.get_prop_value_with_receiver(&prototype, &key, receiver);
            }
        }
        let v = self.prop(o, p)?;
        // Accessors are represented by specially named functions. Most
        // property reads return ordinary methods or data, so avoid converting
        // the key and allocating both candidate names on that hot path.
        let accessor_name = match &v {
            Value::Function(function) => function.name.as_deref(),
            // Native accessors such as `Map.prototype.size` use the same
            // representation as guest getters.
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                Some(name.as_ref())
            }
            _ => None,
        };
        let name_matches = |prefix: &str| -> Result<bool, VmErr> {
            let Some(name) = accessor_name.and_then(|name| name.strip_prefix(prefix)) else {
                return Ok(false);
            };
            // String keys delegate to `get_prop_value_str`; only exotic keys
            // reach this coercion.
            Ok(name == self.property_key(p)?
                || matches!(p, Value::Symbol(_)) && name == self.property_function_name(p)?)
        };
        let is_getter = name_matches("get ")?;
        let is_setter_only = !is_getter && name_matches("set ")?;
        if is_getter {
            return self.call_this(&v, receiver.clone(), vec![]);
        }
        if is_setter_only {
            return Ok(Value::Undefined);
        }
        Ok(v)
    }

    /// Borrowed-key variant of [`get_prop_value`](Self::get_prop_value).
    /// Static member reads (`o.key`) and internal lookups resolve through
    /// `&str` end to end: no key `String` and no key `Value` is allocated.
    /// Proxy targets still allocate the trap key, exactly as before.
    pub(crate) fn get_prop_value_str(&mut self, o: &Value, key: &str) -> Result<Value, VmErr> {
        self.get_prop_value_str_with_receiver(o, key, o)
    }

    fn get_prop_value_str_with_receiver(
        &mut self,
        o: &Value,
        key: &str,
        receiver: &Value,
    ) -> Result<Value, VmErr> {
        self.with_property_get(|vm| vm.get_prop_value_str_with_receiver_inner(o, key, receiver))
    }

    fn get_prop_value_str_with_receiver_inner(
        &mut self,
        o: &Value,
        key: &str,
        receiver: &Value,
    ) -> Result<Value, VmErr> {
        if let Value::RealmGlobal(global) = o {
            return self.with_global_storage(global.clone(), |vm| {
                vm.get_prop_value_str_with_receiver(&Value::GlobalObject, key, receiver)
            });
        }
        if matches!(o, Value::GlobalObject) {
            let record = self
                .persistent_global
                .borrow()
                .global_object()
                .expect("global object record");
            return self.get_prop_value_str_with_receiver(&record, key, receiver);
        }
        let value = self.get_prop_value_str_inner(o, key, receiver)?;
        if matches!(value, Value::Uninitialized) {
            return Err(VmErr::Msg(format!(
                "ReferenceError: Cannot access '{key}' before initialization"
            )));
        }
        Ok(value)
    }
    fn get_prop_value_str_inner(
        &mut self,
        o: &Value,
        key: &str,
        receiver: &Value,
    ) -> Result<Value, VmErr> {
        if let Value::Array(array) = o
            && let Some(index) = crate::value::array_index(key)
            && !array.has_index(index)
        {
            return if let Some(prototype) = self.prototype_of(o) {
                self.get_prop_value_str_with_receiver(prototype.as_ref(), key, receiver)
            } else {
                Ok(Value::Undefined)
            };
        }
        if let Some(proxy) = o.as_proxy() {
            let (target, handler) = proxy.snapshot()?;
            if let Some(trap) = self.proxy_trap(&handler, "get")? {
                let trap_key = Value::String(crate::JsString::from_key(key));
                let trap_key = self.to_property_key(&trap_key)?;

                let result = self.call_this(
                    &trap,
                    handler,
                    vec![target.clone(), trap_key.clone(), receiver.clone()],
                )?;
                return self.validate_proxy_get(&target, &trap_key, result);
            }
            return self.get_prop_value_str_with_receiver(&target, key, receiver);
        }
        if matches!(o, Value::Null | Value::Undefined) {
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
        // Ordinary [[Get]] delegates missing own properties to the prototype's
        // internal operation, retaining the original receiver for accessors.
        if let Some(props) = match o {
            Value::Object { props } => Some(props.clone()),
            _ if !self.seq(o, receiver) => o.exotic_properties(),
            _ => None,
        } {
            let inherited = {
                let own = props.borrow().iter().any(|(name, _)| name == key);
                if own { None } else { props.proto() }
            };
            if let Some(prototype) = inherited
                && matches!(
                    crate::builtins::object::descriptor_for_key_in(
                        self,
                        o,
                        &Value::String(crate::JsString::from_key(key))
                    )?,
                    Value::Undefined
                )
            {
                return self.get_prop_value_str_with_receiver(&prototype, key, receiver);
            }
        }
        let v = self.prop_str(o, key)?;
        // The global object record stores variable values as data. Callable
        // names are not sufficient to turn an ordinary binding into a getter.
        if let Some(cell) = o.property_cell()
            && cell.own_index(key).is_some()
            && !cell.meta.borrow().has_accessors
        {
            return Ok(v);
        }
        let accessor_name = match &v {
            Value::Function(function) => function.name.as_deref(),
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                Some(name.as_ref())
            }
            _ => None,
        };
        let is_getter = accessor_name
            .and_then(|name| name.strip_prefix("get "))
            .is_some_and(|name| name == key);
        let is_setter_only = !is_getter
            && accessor_name
                .and_then(|name| name.strip_prefix("set "))
                .is_some_and(|name| name == key);
        if is_getter {
            return self.call_this(&v, receiver.clone(), vec![]);
        }
        if is_setter_only {
            return Ok(Value::Undefined);
        }
        Ok(v)
    }

    /// Read a property, resolving a live module binding to the value it names.
    #[doc(hidden)]
    pub fn prop(&self, o: &Value, p: &Value) -> Result<Value, VmErr> {
        let value = self.prop_raw(o, p)?;
        // Only a live binding needs a second clone; anything else moves.
        if matches!(value, Value::Binding(_)) {
            Ok(value.deref_binding())
        } else {
            Ok(value)
        }
    }

    /// Borrowed-key variant of [`prop`](Self::prop).
    #[doc(hidden)]
    pub fn prop_str(&self, o: &Value, key: &str) -> Result<Value, VmErr> {
        let value = self.prop_str_raw(o, key)?;
        if matches!(value, Value::Binding(_)) {
            Ok(value.deref_binding())
        } else {
            Ok(value)
        }
    }

    fn prop_raw(&self, o: &Value, p: &Value) -> Result<Value, VmErr> {
        if let Value::Symbol(symbol) = p
            && let Some(properties) = o.exotic_properties()
            && let Some(value) = properties.own_value(&crate::interpreter::symbol_slot_key(symbol))
        {
            return Ok(value);
        }
        // String keys dispatch on the receiver alone in `prop_str_raw`; the
        // match below only sees symbols, numbers, and exotic keys.
        if let Value::String(k) = p {
            return self.prop_str_raw(o, &k.to_key());
        }
        if matches!(p, Value::Symbol(_))
            && let Some(prototype) = self.primitive_reference_prototype(o)
        {
            return self.prop(&prototype, p);
        }
        match (o, p) {
            (Value::GlobalObject, Value::Symbol(_)) => {
                if let Some(prototype) = self.prototype_of(o) {
                    self.prop(&prototype, p)
                } else {
                    Ok(Value::Undefined)
                }
            }
            (Value::Object { props }, Value::Number(index)) => {
                let key = crate::format::number_string(*index);
                if let Some(value) = lookup_chain_found(self, o, &key)? {
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
            (Value::String(s), Value::Number(i)) => {
                let idx = *i as usize;
                Ok(crate::value::str_char_at(s, idx).unwrap_or(Value::Undefined))
            }
            (Value::Class(c), Value::Symbol(symbol)) => lookup_chain(
                self,
                &Value::Object {
                    props: c.statics.clone(),
                },
                &crate::interpreter::symbol_slot_key(symbol),
            ),
            (Value::Function(function), Value::Symbol(symbol)) => lookup_chain(
                self,
                &Value::Object {
                    props: function.properties.clone(),
                },
                &super::symbol_slot_key(symbol),
            ),
            (Value::HostFunction { .. }, Value::Symbol(symbol)) => {
                let key = super::symbol_slot_key(symbol);
                if let Some(value) = lookup_chain_found(self, o, &key)? {
                    return Ok(value);
                }
                let Some(prototype) =
                    FunctionData::default_function_prototype(&self.persistent_global)
                else {
                    return Ok(Value::Undefined);
                };
                lookup_chain(self, &prototype, &key)
            }
            (Value::NativeFunction { .. }, Value::Symbol(symbol)) => {
                let Some(prototype) =
                    FunctionData::default_function_prototype(&self.persistent_global)
                else {
                    return Ok(Value::Undefined);
                };
                lookup_chain(self, &prototype, &super::symbol_slot_key(symbol))
            }

            // Typed array indices and length-like values are resolved on the
            // receiver; methods live on the shared `%TypedArray%.prototype`
            // chain so their identity matches constructor prototypes.
            (Value::TypedArray(view), Value::Number(i)) => {
                if !i.is_finite() || *i < 0.0 || i.fract() != 0.0 {
                    return Ok(Value::Undefined);
                }
                Ok(crate::builtins::read_element(view, *i as usize).unwrap_or(Value::Undefined))
            }
            (Value::TypedArray(_), Value::Symbol(_)) => {
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop(&prototype, p);
                }
                Ok(Value::Undefined)
            }
            (
                Value::ArrayBuffer(_) | Value::SharedArrayBuffer(_) | Value::DataView(_),
                Value::Symbol(_),
            ) => {
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop(&prototype, p);
                }
                Ok(Value::Undefined)
            }
            // Symbol-keyed property access: `arr[Symbol.iterator]`,
            // `str[Symbol.iterator]`, `gen[Symbol.iterator]`.
            (Value::Array(items), Value::Symbol(symbol)) => {
                let key = super::symbol_slot_key(symbol);
                if let Some(value) = items.named_prop(&key) {
                    return Ok(value);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop(&prototype, p);
                }
                if crate::builtins::is_iterator_symbol(p) {
                    Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: array_iter,
                    })
                } else {
                    Ok(Value::Undefined)
                }
            }
            (Value::String(_), Value::Symbol(_)) if crate::builtins::is_iterator_symbol(p) => self
                .primitive_reference_prototype(o)
                .map_or(Ok(Value::Undefined), |prototype| self.prop(&prototype, p)),
            (Value::RegExp(_), Value::Symbol(_)) => {
                if let Some(prototype) = self.prototype_of(o) {
                    self.prop(&prototype, p)
                } else {
                    Ok(Value::Undefined)
                }
            }
            (Value::Generator { .. } | Value::StringIterator { .. }, Value::Symbol(symbol)) => {
                if let Some(value) = lookup_chain_found(self, o, &super::symbol_slot_key(symbol))? {
                    return Ok(value);
                }
                if self.prototype_of(o).is_none() && crate::builtins::is_iterator_symbol(p) {
                    return Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: generator_iter_self,
                    });
                }
                Ok(Value::Undefined)
            }
            // Object symbol-keyed lookup: `obj[Symbol.iterator]` resolves the
            // internal `__symbol_iterator__` property.
            (Value::Object { .. }, Value::Symbol(symbol)) => {
                if let Some(value) = lookup_chain_found(self, o, &super::symbol_slot_key(symbol))? {
                    return Ok(value);
                }
                Ok(Value::Undefined)
            }
            _ => Ok(Value::Undefined),
        }
    }

    /// String-keyed half of [`prop_raw`](Self::prop_raw), dispatching on the
    /// receiver alone. This is the only home of string-key semantics now:
    /// `prop_raw` forwards every `Value::String` key here, and borrowed-key
    /// callers arrive via [`prop_str`](Self::prop_str) without allocating.
    fn prop_str_raw(&self, o: &Value, k: &str) -> Result<Value, VmErr> {
        if let Some(properties) = o.exotic_properties()
            && let Some(value) = properties.own_value(k)
        {
            return Ok(value);
        }
        if let Some(prototype) = self.primitive_reference_prototype(o) {
            if let Value::String(string) = o {
                if k == "length" {
                    return Ok(Value::Number(crate::value::str_char_len(string)));
                }
                if let Some(index) = crate::value::array_index(k)
                    && let Some(character) = crate::value::str_char_at(string, index)
                {
                    return Ok(character);
                }
            }
            return self.prop_str(&prototype, k);
        }
        match o {
            // `window.x` / `globalThis.x` / `self.x` read a real global.
            Value::GlobalObject => {
                if let Some((value, _)) = self.global_property(k) {
                    return Ok(value);
                }
                if self.global_keys().iter().any(|key| key == k) {
                    return Ok(Value::Undefined);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(Value::Undefined)
            }
            Value::Object { props } => {
                if let Some(value) = lookup_chain_found(self, o, k)? {
                    return Ok(value);
                }
                let boxed = props.meta.borrow().boxed_primitive.clone();
                Ok(boxed
                    .map(|primitive| boxed_primitive_property(&primitive, k))
                    .unwrap_or(Value::Undefined))
            }
            Value::Array(items) => {
                if k == "length" {
                    Ok(Value::Number(items.borrow().len() as f64))
                } else if let Some(idx) = crate::value::array_index(k) {
                    let items = items.borrow();
                    if idx < items.len() {
                        Ok(items[idx].clone())
                    } else {
                        Ok(Value::Undefined)
                    }
                } else if let Some(value) = items.named_prop(k) {
                    Ok(value)
                } else {
                    if let Some(prototype) = self.prototype_of(o) {
                        return self.prop_str(&prototype, k);
                    }
                    Ok(crate::builtins::array_method(k).unwrap_or(Value::Undefined))
                }
            }
            Value::String(s) => {
                if k == "length" {
                    Ok(Value::Number(crate::value::str_char_len(s)))
                } else if k == "__symbol_iterator__" {
                    Ok(Value::NativeFunction {
                        name: "[Symbol.iterator]".into(),
                        callable: string_iter,
                    })
                } else if let Ok(idx) = k.parse::<usize>() {
                    Ok(crate::value::str_char_at(s, idx).unwrap_or(Value::Undefined))
                } else if let Some(m) = crate::builtins::string_method(k) {
                    Ok(m)
                } else {
                    Ok(Value::Undefined)
                }
            }
            Value::Number(_) => Ok(crate::builtins::number_method(k).unwrap_or(Value::Undefined)),
            Value::Promise { .. } => {
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(crate::builtins::promise_method(k).unwrap_or(Value::Undefined))
            }
            Value::Class(c) => lookup_chain(
                self,
                &Value::Object {
                    props: c.statics.clone(),
                },
                k,
            ),
            Value::Function(function) => {
                if k == "prototype" {
                    return Ok(function.prototype_value(o));
                }
                function.ensure_name_length_properties();
                if let Some(value) = lookup_chain_found(
                    self,
                    &Value::Object {
                        props: function.properties.clone(),
                    },
                    k,
                )? {
                    return Ok(value);
                }
                Ok(match k {
                    // These values are inherited from Function.prototype
                    // when a callable's configurable own property is deleted.
                    "name" => Value::String((String::new()).into()),
                    "length" => Value::Number(0.0),
                    _ => crate::builtins::function_method(k).unwrap_or(Value::Undefined),
                })
            }
            Value::Generator { .. } => {
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
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
            Value::StringIterator { .. } => {
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                if k == "next" {
                    Ok(Value::NativeFunction {
                        name: "next".into(),
                        callable: string_iter_next,
                    })
                } else {
                    Ok(Value::Undefined)
                }
            }
            Value::NativeFunction { name, .. } => {
                // Well-known symbols and static methods on `Symbol`. A native
                // function cannot carry properties, so they are resolved here.
                if name.as_ref() == "Symbol" {
                    if let Some(symbol) = crate::builtins::well_known(k) {
                        return Ok(symbol);
                    }
                    match k {
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
            Value::HostFunction { .. } => {
                if let Some(value) = lookup_chain_found(self, o, k)? {
                    return Ok(value);
                }
                Ok(match k {
                    "name" => Value::String((String::new()).into()),
                    "length" => Value::Number(0.0),
                    _ => crate::builtins::function_method(k).unwrap_or(Value::Undefined),
                })
            }
            Value::TypedArray(view) => {
                if let Some(index) = crate::builtins::canonical_numeric_index(k) {
                    return Ok(if crate::builtins::valid_integer_index(view, index) {
                        crate::builtins::read_element(view, index as usize)
                            .unwrap_or(Value::Undefined)
                    } else {
                        Value::Undefined
                    });
                }
                if let Some(value) = crate::builtins::typed_member(view, k) {
                    return Ok(value);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(Value::Undefined)
            }
            Value::ArrayBuffer(bytes) => {
                if let Some(value) = crate::builtins::array_buffer_member(bytes, k) {
                    return Ok(value);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(Value::Undefined)
            }
            Value::SharedArrayBuffer(bytes) => {
                if let Some(value) = crate::builtins::shared_array_buffer_member(bytes, k) {
                    return Ok(value);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(Value::Undefined)
            }
            Value::DataView(view) => {
                if view.is_out_of_bounds() && matches!(k, "byteLength" | "byteOffset") {
                    return Err(VmErr::Msg(
                        "TypeError: Cannot access a DataView backed by a detached ArrayBuffer"
                            .to_string(),
                    ));
                }
                if let Some(value) = crate::builtins::data_view_member(view, k) {
                    return Ok(value);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(Value::Undefined)
            }
            Value::Date(_) => {
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(crate::builtins::date_member(k).unwrap_or(Value::Undefined))
            }
            Value::BigInt(_) => Ok(crate::builtins::bigint_method(k).unwrap_or(Value::Undefined)),
            Value::RegExp(data) => {
                if let Some(value) = crate::builtins::regexp_member(data, k) {
                    return Ok(value);
                }
                if let Some(prototype) = self.prototype_of(o) {
                    return self.prop_str(&prototype, k);
                }
                Ok(Value::Undefined)
            }
            Value::Symbol(symbol) => match k {
                "description" => Ok(symbol
                    .description
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Undefined)),
                other => Ok(crate::builtins::symbol_method(other).unwrap_or(Value::Undefined)),
            },
            // Internal errors surface to guest `catch` blocks as error objects
            // with readable `name`/`message` properties.
            Value::Error(e) => match k {
                "message" => Ok(Value::String(e.message.clone())),
                "name" => Ok(Value::String(e.name.clone())),
                "stack" => Ok(Value::String(e.stack.clone())),
                "code" => Ok(e.code.clone().map_or(Value::Undefined, Value::String)),
                _ => match self.prototype_of(o) {
                    Some(prototype) => self.prop_str(&prototype, k),
                    None => Ok(Value::Undefined),
                },
            },
            // Booleans, null, undefined, proxies (handled by the caller), and
            // live bindings carry no string-keyed properties.
            _ => Ok(Value::Undefined),
        }
    }
}

/// Walk an object's prototype chain looking for `key`, bounded by
/// [`MAX_PROTOTYPE_DEPTH`](crate::value::MAX_PROTOTYPE_DEPTH) so a guest-built
/// cycle spends bounded time instead of hanging.
fn lookup_chain(interp: &Interpreter, o: &Value, key: &str) -> Result<Value, VmErr> {
    Ok(lookup_chain_found(interp, o, key)?.unwrap_or(Value::Undefined))
}

/// Look up an object property without conflating a missing property with an
/// own or inherited property whose value is `undefined`.
fn lookup_chain_found(interp: &Interpreter, o: &Value, key: &str) -> Result<Option<Value>, VmErr> {
    // Borrow the receiver until the walk actually descends: own-property hits
    // (the common case) never clone it.
    let mut descended: Option<Value> = None;
    for _ in 0..=crate::value::MAX_PROTOTYPE_DEPTH {
        let node = descended.as_ref().unwrap_or(o);
        let props = match node {
            Value::Object { props } => props.clone(),
            Value::Array(array) => {
                if let Some(value) = array.named_prop(key) {
                    return Ok(Some(value.deref_binding()));
                }
                // Array indices are handled by the caller before this walk.
                // Named values and length live outside `ObjectCell`.
                let Some(proto) = interp.prototype_of(node) else {
                    return Ok(None);
                };
                descended = Some(proto.as_ref().clone());
                continue;
            }
            Value::Class(class) => class.statics.clone(),
            Value::Function(function) => function.properties.clone(),
            Value::HostFunction { properties, .. } => properties.clone(),
            _ => match node.exotic_properties() {
                Some(properties) => properties,
                None => return Ok(None),
            },
        };
        if let Some(value) = props.own_value(key) {
            return Ok(Some(value));
        }
        let Some(next) = interp.prototype_of(node) else {
            return Ok(None);
        };
        descended = Some(next.as_ref().clone());
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
                return Value::Number(crate::value::str_char_len(value));
            }
            if let Some(index) = crate::value::array_index(key) {
                return crate::value::str_char_at(value, index).unwrap_or(Value::Undefined);
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
    let text: crate::JsString = match boxed_primitive_receiver(&this)? {
        BoxedPrimitive::Bool(value) => value.to_string().into(),
        BoxedPrimitive::Number(value) => crate::format::number_string(value).into(),
        BoxedPrimitive::String(value) => value,
        BoxedPrimitive::Symbol(value) => value.to_display(),
        BoxedPrimitive::BigInt(value) => value.to_decimal().into(),
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

pub(crate) fn install_iterator_intrinsics(environment: &mut super::Environment) {
    let object_prototype = environment
        .get("Object")
        .and_then(|constructor| constructor.get_prop("prototype"));
    let function_prototype = environment
        .get("Function")
        .and_then(|constructor| constructor.get_prop("prototype"));
    let iterator = Value::object_with_proto(vec![], object_prototype.map(Rc::new));
    let method = crate::builtins::native_method(
        "[Symbol.iterator]",
        0,
        generator_iter_self,
        function_prototype.clone(),
    );
    iterator
        .set_prop(super::SYMBOL_ITERATOR_SLOT.into(), method)
        .expect("iterator method");
    if let Value::Object { props } = &iterator {
        if let Some(Value::Symbol(ref symbol)) = crate::builtins::well_known("iterator") {
            props
                .meta
                .borrow_mut()
                .set_symbol_key(super::SYMBOL_ITERATOR_SLOT, symbol.clone());
        }
        props.meta.borrow_mut().set_attrs(
            super::SYMBOL_ITERATOR_SLOT,
            crate::value::PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    }
    environment.install_intrinsic("%IteratorPrototype%", iterator.clone());
    let async_iterator = Value::object_with_proto(
        vec![],
        environment
            .get("Object")
            .and_then(|constructor| constructor.get_prop("prototype"))
            .map(Rc::new),
    );
    let Value::Symbol(ref async_symbol) =
        crate::builtins::well_known("asyncIterator").expect("asyncIterator")
    else {
        unreachable!()
    };
    let async_key = super::symbol_slot_key(async_symbol);
    async_iterator
        .set_prop(
            async_key.clone(),
            crate::builtins::native_method(
                "[Symbol.asyncIterator]",
                0,
                generator_iter_self,
                function_prototype.clone(),
            ),
        )
        .expect("async iterator method");
    if let Value::Object { props } = &async_iterator {
        let mut meta = props.meta.borrow_mut();
        meta.set_symbol_key(&async_key, async_symbol.clone());
        meta.set_attrs(
            &async_key,
            crate::value::PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    }
    environment.install_intrinsic("%AsyncIteratorPrototype%", async_iterator.clone());
    let async_from_sync = Value::object_with_proto(vec![], Some(Rc::new(async_iterator.clone())));
    for (name, callable) in [
        (
            "next",
            super::async_from_sync::next as crate::builtins::NativeFn,
        ),
        ("return", super::async_from_sync::return_),
        ("throw", super::async_from_sync::throw),
    ] {
        async_from_sync
            .set_prop(
                name.into(),
                crate::builtins::native_method(name, 1, callable, function_prototype.clone()),
            )
            .expect("async-from-sync method");
        async_from_sync
            .property_cell()
            .expect("intrinsic object")
            .meta
            .borrow_mut()
            .set_attrs(
                name,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
    }
    environment.install_intrinsic("%AsyncFromSyncIteratorPrototype%", async_from_sync);
    for (kind, tag, methods) in [
        (
            "%ArrayIteratorPrototype%",
            "Array Iterator",
            vec![("next", 0, array_iter_next as crate::builtins::NativeFn)],
        ),
        (
            "%StringIteratorPrototype%",
            "String Iterator",
            vec![("next", 0, string_iter_next)],
        ),
        (
            "%GeneratorPrototype%",
            "Generator",
            vec![
                ("next", 1, super::call::generator_next),
                ("return", 1, super::call::generator_return),
                ("throw", 1, super::call::generator_throw),
            ],
        ),
        (
            "%AsyncGeneratorPrototype%",
            "AsyncGenerator",
            vec![
                ("next", 1, super::async_generator::next),
                ("return", 1, super::async_generator::return_),
                ("throw", 1, super::async_generator::throw),
            ],
        ),
    ] {
        let parent = if kind == "%AsyncGeneratorPrototype%" {
            &async_iterator
        } else {
            &iterator
        };
        let prototype = Value::object_with_proto(vec![], Some(Rc::new(parent.clone())));
        for (name, length, callable) in methods {
            prototype
                .set_prop(
                    name.into(),
                    crate::builtins::native_method(
                        name,
                        length,
                        callable,
                        function_prototype.clone(),
                    ),
                )
                .expect("iterator method");
            if let Value::Object { props } = &prototype {
                props.meta.borrow_mut().set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            }
        }
        let Value::Symbol(ref symbol) =
            crate::builtins::well_known("toStringTag").expect("toStringTag")
        else {
            unreachable!()
        };
        let key = super::symbol_slot_key(symbol);
        prototype
            .set_prop(key.clone(), Value::String(tag.into()))
            .expect("iterator tag");
        if let Value::Object { props } = &prototype {
            let mut meta = props.meta.borrow_mut();
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
        let constructor_kind = match kind {
            "%GeneratorPrototype%" => Some("%GeneratorFunction%"),
            "%AsyncGeneratorPrototype%" => Some("%AsyncGeneratorFunction%"),
            _ => None,
        };
        if let Some(constructor_kind) = constructor_kind
            && let Some(function_prototype) = environment
                .intrinsic(constructor_kind)
                .and_then(|constructor| constructor.get_prop("prototype"))
        {
            for (object, key, value) in [
                (&prototype, "constructor", function_prototype.clone()),
                (&function_prototype, "prototype", prototype.clone()),
            ] {
                object
                    .set_prop(key.into(), value)
                    .expect("generator intrinsic link");
                if let Some(properties) = object.property_cell() {
                    properties.meta.borrow_mut().set_attrs(
                        key,
                        crate::value::PropAttrs {
                            writable: false,
                            enumerable: false,
                            configurable: true,
                        },
                    );
                }
            }
        }
        environment.install_intrinsic(kind, prototype);
    }
    if let Some(string_prototype) = environment
        .get("String")
        .and_then(|constructor| constructor.get_prop("prototype"))
    {
        string_prototype
            .set_prop(
                super::SYMBOL_ITERATOR_SLOT.into(),
                crate::builtins::native_method(
                    "[Symbol.iterator]",
                    0,
                    string_iter,
                    function_prototype,
                ),
            )
            .expect("string iterator method");
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
pub(crate) fn array_iter(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    array_iter_with_kind(this, "values")
}

pub(crate) fn array_iter_with_kind(
    source: super::Value,
    kind: &str,
) -> Result<super::Value, crate::error::VmErr> {
    let iterator = super::Value::object(vec![
        ("__items__".into(), source),
        ("__cursor__".into(), super::Value::Number(0.0)),
        ("__kind__".into(), super::Value::String(kind.into())),
        (
            "next".into(),
            super::Value::NativeFunction {
                name: "next".into(),
                callable: array_iter_next,
            },
        ),
        (
            crate::interpreter::SYMBOL_ITERATOR_SLOT.into(),
            super::Value::NativeFunction {
                name: "[Symbol.iterator]".into(),
                callable: string_iter_self,
            },
        ),
    ]);
    if let super::Value::Object { props } = &iterator {
        if let Some(prototype) = super::realm::allocation_global()
            .and_then(|global| global.borrow().intrinsic("%ArrayIteratorPrototype%"))
        {
            props.set_proto(Some(Rc::new(prototype)));
            props
                .borrow_mut()
                .retain(|(key, _)| key != "next" && key != super::SYMBOL_ITERATOR_SLOT);
        }
        for key in [
            "__items__",
            "__cursor__",
            "__kind__",
            "next",
            crate::interpreter::SYMBOL_ITERATOR_SLOT,
        ] {
            props.meta.borrow_mut().set_attrs(
                key,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
        }
    }
    Ok(iterator)
}

/// Read the source at each step. Array iterators observe changes to both
/// values and length, but remain exhausted after the first done result.
fn array_iter_next(
    interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    let source = this
        .get_prop("__items__")
        .unwrap_or(super::Value::Undefined);
    if matches!(source, super::Value::Undefined) {
        return Ok(super::call::iter_result(super::Value::Undefined, true));
    }
    if matches!(&source, Value::TypedArray(view) if view.is_out_of_bounds()) {
        return Err(crate::error::VmErr::Msg(
            "TypeError: Cannot iterate a detached typed array".into(),
        ));
    }
    let length = match &source {
        Value::TypedArray(view) => view.effective_length() as f64,
        _ => {
            let length = interp.get_prop_value_str(&source, "length")?;
            interp.ecmascript_to_number(&length)?
        }
    };
    let cursor = match this.get_prop("__cursor__") {
        Some(super::Value::Number(n)) => n as usize,
        _ => 0,
    };
    if cursor as f64 >= length || length.is_nan() {
        this.set_prop("__items__".into(), super::Value::Undefined)?;
        return Ok(super::call::iter_result(super::Value::Undefined, true));
    }
    if cursor >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum array iteration length exceeded",
        ));
    }
    this.set_prop(
        "__cursor__".into(),
        super::Value::Number((cursor + 1) as f64),
    )?;
    let kind = this.get_prop("__kind__");
    let index = super::Value::Number(cursor as f64);
    let value = if matches!(&kind, Some(super::Value::String(kind)) if kind == "keys") {
        index
    } else {
        let value = interp.get_prop_value_str(&source, &cursor.to_string())?;
        if matches!(&kind, Some(super::Value::String(kind)) if kind == "entries") {
            super::Value::array(vec![index, value])
        } else {
            value
        }
    };
    Ok(super::call::iter_result(value, false))
}

/// `[Symbol.iterator]()` on a string returns a character iterator.
fn string_iter(
    _interp: &mut super::Interpreter,
    this: super::Value,
    _args: Vec<super::Value>,
) -> Result<super::Value, crate::error::VmErr> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let source: crate::JsString = match &this {
        super::Value::String(s) => s.clone(),
        _ => crate::JsString::default(),
    };

    Ok(super::Value::StringIterator {
        inner: Rc::new(RefCell::new(crate::value::StringIteratorData {
            properties: {
                let properties = Value::instance_properties();
                let prototype = super::realm::allocation_global()
                    .and_then(|global| global.borrow().intrinsic("%StringIteratorPrototype%"));
                properties.set_proto(prototype.map(Rc::new));
                properties
            },
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
    if state.cursor >= state.source.len() {
        return Ok(super::call::iter_result(super::Value::Undefined, true));
    }
    let start = state.cursor;
    let count = state
        .source
        .code_point_at(start)
        .map(|cp| if cp > 0xFFFF { 2 } else { 1 })
        .unwrap_or(1);
    state.cursor += count;
    Ok(super::call::iter_result(
        super::Value::String(state.source.slice(start, start + count)),
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
        Value::Proxy(proxy) => has_default_object_prototype(&proxy.target_for_inspection()),
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

#[cfg(test)]
mod boxed_primitive_prototype_tests {
    use std::process::Command;

    use crate::interpreter::Interpreter;
    use crate::value::Value;

    #[test]
    fn boxed_primitives_use_their_constructor_prototypes() {
        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter
            .eval_source(
                "[Object(false), Object(12), Object('abc'), Object(Symbol('x')), Object(13n)].map((value, index) => Object.getPrototypeOf(value) === [Boolean.prototype, Number.prototype, String.prototype, Symbol.prototype, BigInt.prototype][index]).every(Boolean)",
            )
            .unwrap();
        assert!(
            matches!(result, Value::Bool(true)),
            "boxed primitive prototype check returned {result:?}"
        );
    }

    #[test]
    fn primitive_constructor_fixture_matches_node_and_bun() {
        let fixture = r#"(() => {
  const symbol = Symbol('x');
  const bigint = 13n;
  const boolean = new Boolean(false);
  const number = new Number(12);
  const string = new String('abc');
  const boxedSymbol = Object(symbol);
  const boxedBigInt = Object(bigint);
  const throws = (callback) => {
    try { callback(); return false; }
    catch (error) { return error instanceof TypeError; }
  };
  return JSON.stringify({
    symbolType: typeof Symbol,
    symbolRegistry: Symbol.keyFor(Symbol.for('registry-key')),
    wellKnownSymbol: Symbol.iterator === Symbol.iterator,
    boolean: [typeof boolean, Object.getPrototypeOf(boolean) === Boolean.prototype,
      boolean.valueOf(), boolean.toString()],
    number: [typeof number, Object.getPrototypeOf(number) === Number.prototype,
      number.valueOf(), number.toFixed(1)],
    string: [typeof string, Object.getPrototypeOf(string) === String.prototype,
      string.valueOf(), string.toUpperCase(), string.length],
    symbol: [Object.getPrototypeOf(boxedSymbol) === Symbol.prototype,
      boxedSymbol.valueOf() === symbol, boxedSymbol.description,
      Object.getPrototypeOf(Symbol) === Function.prototype],
    bigint: [Object.getPrototypeOf(boxedBigInt) === BigInt.prototype,
      boxedBigInt.valueOf() === bigint, boxedBigInt.toString()],
    json: [JSON.stringify(boolean), JSON.stringify(number), JSON.stringify(string),
      JSON.stringify(boxedSymbol)],
    customJson: JSON.stringify(Object.assign(new Number(4), {toJSON() { return 17; }})),
    bigintJsonThrows: throws(() => JSON.stringify(boxedBigInt)),
    rejectsConstructors: [throws(() => new Symbol()), throws(() => new BigInt(1))],
  });
})()"#;
        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter.eval_source(fixture).unwrap();
        let Value::String(ref result) = result else {
            panic!("primitive constructor fixture returned {result:?}");
        };
        let expected: serde_json::Value = serde_json::from_str(result).unwrap();
        assert_eq!(expected["symbolType"], "function");
        assert_eq!(expected["symbolRegistry"], "registry-key");
        assert_eq!(expected["wellKnownSymbol"], true);
        assert_eq!(
            expected["boolean"],
            serde_json::json!(["object", true, false, "false"])
        );
        assert_eq!(
            expected["number"],
            serde_json::json!(["object", true, 12, "12.0"])
        );
        assert_eq!(
            expected["string"],
            serde_json::json!(["object", true, "abc", "ABC", 3])
        );
        assert_eq!(
            expected["symbol"],
            serde_json::json!([true, true, "x", true])
        );
        assert_eq!(expected["bigint"], serde_json::json!([true, true, "13"]));
        assert_eq!(
            expected["json"],
            serde_json::json!(["false", "12", "\"abc\"", "{}"])
        );
        assert_eq!(expected["customJson"], "17");
        assert_eq!(expected["bigintJsonThrows"], true);
        assert_eq!(
            expected["rejectsConstructors"],
            serde_json::json!([true, true])
        );

        for runtime in ["node", "bun"] {
            let Ok(reference) = Command::new(runtime)
                .args(["-e", &format!("process.stdout.write({fixture})")])
                .output()
            else {
                continue;
            };
            if !reference.status.success() {
                eprintln!(
                    "skipping {runtime} primitive comparison: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                continue;
            }
            let actual: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(expected, actual, "{runtime} primitive behavior differed");
        }
    }

    #[test]
    fn object_prototype_methods_match_node_and_bun() {
        let fixture = r#"JSON.stringify((() => {
  const plain = { own: 1 };
  const customTag = { [Symbol.toStringTag]: 'Widget' };
  const localized = { toString() { return 'localized'; } };
  const accessors = {};
  let assigned = 0;
  accessors.__defineGetter__('answer', () => 42);
  accessors.__defineSetter__('value', value => { assigned = value; });
  accessors.value = 9;
  const customPrototype = { inherited: true };
  const child = {};
  child.__proto__ = customPrototype;
  const arrayChild = [];
  arrayChild.__proto__ = customPrototype;
  const boxedNumber = Object.prototype.valueOf.call(12);
  return {
    prototypeNames: Object.getOwnPropertyNames(Object.prototype).sort(),
    objectString: Object.prototype.toString.call(plain),
    nullString: Object.prototype.toString.call(null),
    undefinedString: Object.prototype.toString.call(undefined),
    arrayString: Object.prototype.toString.call([]),
    dateString: Object.prototype.toString.call(new Date(0)),
    customTag: Object.prototype.toString.call(customTag),
    localeString: localized.toLocaleString(),
    valueOf: [plain.valueOf() === plain, typeof boxedNumber, boxedNumber.valueOf()],
    ownProperty: [plain.hasOwnProperty('own'), plain.hasOwnProperty('missing'),
      globalThis.hasOwnProperty('Object')],
    enumerable: [plain.propertyIsEnumerable('own'), plain.propertyIsEnumerable('missing'),
      Object.prototype.propertyIsEnumerable.call('abc', '0')],
    prototype: [Object.getPrototypeOf(plain).isPrototypeOf(plain),
      Object.prototype.isPrototypeOf(plain)],
    accessors: [accessors.answer, assigned,
      accessors.__lookupGetter__('answer') === Object.getOwnPropertyDescriptor(accessors, 'answer').get,
      accessors.__lookupSetter__('value') === Object.getOwnPropertyDescriptor(accessors, 'value').set],
    protoAccessor: [child.__proto__ === customPrototype, child.inherited,
      arrayChild.__proto__ === customPrototype, arrayChild.inherited,
      Object.getOwnPropertyDescriptor(Object.prototype, '__proto__').enumerable],
  };
})())"#;

        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter.eval_source(fixture).unwrap();
        let Value::String(ref result) = result else {
            panic!("Object.prototype fixture returned {result:?}");
        };
        let expected: serde_json::Value = serde_json::from_str(result).unwrap();
        assert_eq!(expected["objectString"], "[object Object]");
        assert_eq!(expected["customTag"], "[object Widget]");
        assert_eq!(expected["localeString"], "localized");
        assert_eq!(
            expected["ownProperty"],
            serde_json::json!([true, false, true])
        );
        assert_eq!(
            expected["accessors"],
            serde_json::json!([42, 9, true, true])
        );
        assert_eq!(
            expected["protoAccessor"],
            serde_json::json!([true, true, true, true, false])
        );

        for runtime in ["node", "bun"] {
            let Ok(reference) = Command::new(runtime)
                .args(["-e", &format!("process.stdout.write({fixture})")])
                .output()
            else {
                continue;
            };
            if !reference.status.success() {
                eprintln!(
                    "skipping {runtime} Object.prototype comparison: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                continue;
            }
            let actual: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(
                expected, actual,
                "{runtime} Object.prototype behavior differed"
            );
        }
    }
}
