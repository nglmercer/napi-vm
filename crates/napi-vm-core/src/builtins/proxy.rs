//! `Proxy`: a target object wrapped by a handler whose traps intercept the
//! fundamental operations.
//!
//! Slots are captured before trap access and cleared by revocation. The
//! shared Interpreter operations here also serve ordinary objects and the
//! Object/Reflect APIs. Descriptor and key operations are being consolidated
//! separately; this module does not claim complete Phase 3 invariants.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{ProxyData, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("Proxy") {
        super::make_callable(&namespace, super::require_new, Some(new_proxy));
        let prototype = e
            .get("Function")
            .and_then(|function| function.get_prop("prototype"));
        super::object::define_property(
            &namespace,
            "revocable",
            &Value::object(vec![
                (
                    "value".into(),
                    super::native_method("revocable", 2, proxy_revocable, prototype),
                ),
                ("writable".into(), Value::Bool(true)),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .expect("Proxy.revocable");
    }
}

fn new_proxy(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let handler = a.get(1).cloned().unwrap_or(Value::Undefined);
    if !crate::interpreter::call::is_js_object(&target) {
        return Err(VmErr::Msg(
            "TypeError: Cannot create proxy with a non-object as target".to_string(),
        ));
    }
    if !crate::interpreter::call::is_js_object(&handler) {
        return Err(VmErr::Msg(
            "TypeError: Cannot create proxy with a non-object as handler".to_string(),
        ));
    }
    Ok(Value::Proxy(Rc::new(ProxyData::new(target, handler))))
}

fn proxy_revocable(
    interp: &mut Interpreter,
    _: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let proxy = new_proxy(interp, Value::Undefined, arguments)?;
    let state = Value::object(Vec::new());
    let Value::Proxy(data) = &proxy else {
        unreachable!()
    };
    state
        .property_cell()
        .expect("revoker state")
        .meta
        .borrow_mut()
        .revocable_proxy = Some(data.clone());
    let prototype = crate::interpreter::realm::allocation_global()
        .and_then(|realm| crate::value::FunctionData::default_function_prototype(&realm));
    let revoke = super::bound_native_method("", 0, proxy_revoke, prototype, state);
    Ok(Value::object(vec![
        ("proxy".into(), proxy),
        ("revoke".into(), revoke),
    ]))
}

fn proxy_revoke(_: &mut Interpreter, state: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let proxy = state
        .property_cell()
        .and_then(|cell| cell.meta.borrow_mut().revocable_proxy.take());
    if let Some(proxy) = proxy {
        proxy.revoke();
    }
    Ok(Value::Undefined)
}

impl Interpreter {
    /// The handler's trap named `name`, if it defines one.
    pub(crate) fn proxy_trap(
        &mut self,
        handler: &Value,
        name: &str,
    ) -> Result<Option<Value>, VmErr> {
        self.get_method(handler, &Value::String(name.into()))
    }

    /// Convert a property operand to its internal PropertyKey while retaining
    /// symbols for proxy traps. The trap receives the original symbol value,
    /// rather than a string description of it.
    pub(crate) fn proxy_property_key(&mut self, key: &Value) -> Result<Value, VmErr> {
        match key {
            Value::Symbol(_) => Ok(key.clone()),
            _ => Ok(Value::String(crate::JsString::from_key(
                &self.property_key(key)?,
            ))),
        }
    }

    pub(crate) fn is_extensible(&mut self, value: &Value) -> Result<bool, VmErr> {
        if let Value::Proxy(proxy) = value {
            let (target, handler) = proxy.snapshot()?;
            let Some(trap) = self.proxy_trap(&handler, "isExtensible")? else {
                return self.is_extensible(&target);
            };
            let result = self
                .call_this(&trap, handler, vec![target.clone()])?
                .is_truthy();
            if result != self.is_extensible(&target)? {
                return Err(VmErr::Msg(
                    "TypeError: Proxy isExtensible result differs from its target".into(),
                ));
            }
            return Ok(result);
        }
        Ok(match value {
            Value::Array(array) => !array.meta.borrow().non_extensible,
            _ => value
                .property_cell()
                .is_some_and(|properties| !properties.meta.borrow().non_extensible),
        })
    }

    pub(crate) fn prevent_extensions(&mut self, value: &Value) -> Result<bool, VmErr> {
        if let Value::Proxy(proxy) = value {
            let (target, handler) = proxy.snapshot()?;
            let Some(trap) = self.proxy_trap(&handler, "preventExtensions")? else {
                return self.prevent_extensions(&target);
            };
            if !self
                .call_this(&trap, handler, vec![target.clone()])?
                .is_truthy()
            {
                return Ok(false);
            }
            if self.is_extensible(&target)? {
                return Err(VmErr::Msg(
                    "TypeError: Proxy preventExtensions target is still extensible".into(),
                ));
            }
            return Ok(true);
        }
        if let Value::Array(array) = value {
            array.meta.borrow_mut().non_extensible = true;
        } else if let Some(properties) = value.property_cell() {
            properties.meta.borrow_mut().non_extensible = true;
        }
        Ok(true)
    }

    pub(crate) fn set_prototype_of(
        &mut self,
        value: &Value,
        requested: &Value,
    ) -> Result<bool, VmErr> {
        if let Value::Proxy(proxy) = value {
            let (target, handler) = proxy.snapshot()?;
            let Some(trap) = self.proxy_trap(&handler, "setPrototypeOf")? else {
                return self.set_prototype_of(&target, requested);
            };
            if !self
                .call_this(&trap, handler, vec![target.clone(), requested.clone()])?
                .is_truthy()
            {
                return Ok(false);
            }
            if !self.is_extensible(&target)?
                && !crate::interpreter::strict_equals(requested, &self.get_prototype_of(&target)?)
            {
                return Err(VmErr::Msg(
                    "TypeError: Proxy setPrototypeOf violates the target prototype".into(),
                ));
            }
            return Ok(true);
        }
        let old = self.get_prototype_of(value)?;
        if crate::interpreter::strict_equals(&old, requested) {
            return Ok(true);
        }
        if !self.is_extensible(value)? {
            return Ok(false);
        }
        let mut current = requested.clone();
        for depth in 0..crate::value::MAX_PROTOTYPE_DEPTH {
            if matches!(current, Value::Null | Value::Proxy(_)) {
                break;
            }
            if crate::interpreter::strict_equals(value, &current) {
                return Ok(false);
            }
            if depth + 1 == crate::value::MAX_PROTOTYPE_DEPTH {
                return Err(crate::value::limit_err("Maximum prototype depth exceeded"));
            }
            current = self.get_prototype_of(&current)?;
        }
        let prototype = (!matches!(requested, Value::Null)).then(|| Rc::new(requested.clone()));
        if let Value::Array(array) = value {
            array.set_proto(prototype);
        } else if let Some(properties) = value.property_cell() {
            properties.set_proto(prototype);
        }
        Ok(true)
    }

    /// Apply the Proxy `[[GetPrototypeOf]]` operation, including the
    /// non-extensible-target invariant. Ordinary objects use the stored or
    /// realm-default prototype without guest re-entry.
    pub(crate) fn get_prototype_of(&mut self, value: &Value) -> Result<Value, VmErr> {
        let Value::Proxy(proxy) = value else {
            return Ok(self
                .prototype_of(value)
                .map_or(Value::Null, |prototype| prototype.as_ref().clone()));
        };
        let (target, handler) = proxy.snapshot()?;

        let Some(trap) = self.proxy_trap(&handler, "getPrototypeOf")? else {
            return self.get_prototype_of(&target);
        };
        let trap_result = self.call_this(&trap, handler, vec![target.clone()])?;
        if !matches!(trap_result, Value::Null) && !is_proxy_object(&trap_result) {
            return Err(VmErr::Msg(
                "TypeError: Proxy getPrototypeOf trap must return an object or null".into(),
            ));
        }
        if !self.is_extensible(&target)? {
            let target_prototype = self.get_prototype_of(&target)?;
            if !crate::interpreter::strict_equals(&trap_result, &target_prototype) {
                return Err(VmErr::Msg(
                    "TypeError: Proxy getPrototypeOf trap must return the target's actual prototype when the target is non-extensible".into(),
                ));
            }
        }
        Ok(trap_result)
    }
}

fn is_proxy_object(value: &Value) -> bool {
    !matches!(
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
    )
}
