//! `Proxy`: a target object wrapped by a handler whose traps intercept the
//! fundamental operations.
//!
//! Only the traps this interpreter can route are supported — `get`, `set`,
//! `has`, `deleteProperty`, `ownKeys`, `getPrototypeOf`, `apply` and
//! `construct`. An operation with no trap falls through to the target, which
//! is what makes an empty handler transparent.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{ProxyData, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("Proxy") {
        super::make_callable(&namespace, new_proxy, None);
    }
}

fn new_proxy(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let handler = a.get(1).cloned().unwrap_or(Value::Undefined);
    if !matches!(
        target,
        Value::Object { .. }
            | Value::Array(_)
            | Value::Function(_)
            | Value::Class(_)
            | Value::Proxy(_)
            // The global scope is an ordinary object to every member and
            // prototype path; only this allowlist excluded it.
            | Value::GlobalObject
    ) {
        return Err(VmErr::Msg(
            "TypeError: Cannot create proxy with a non-object as target".to_string(),
        ));
    }
    if !matches!(handler, Value::Object { .. }) {
        return Err(VmErr::Msg(
            "TypeError: Cannot create proxy with a non-object as handler".to_string(),
        ));
    }
    Ok(Value::Proxy(Rc::new(ProxyData { target, handler })))
}

impl Interpreter {
    /// The handler's trap named `name`, if it defines one.
    pub(crate) fn proxy_trap(&mut self, proxy: &Rc<ProxyData>, name: &str) -> Option<Value> {
        let trap = self.member(&proxy.handler, name).ok()?;
        matches!(
            trap,
            Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
        )
        .then_some(trap)
    }

    /// Convert a property operand to its internal PropertyKey while retaining
    /// symbols for proxy traps. The trap receives the original symbol value,
    /// rather than a string description of it.
    pub(crate) fn proxy_property_key(&mut self, key: &Value) -> Result<Value, VmErr> {
        match key {
            Value::Symbol(_) => Ok(key.clone()),
            _ => Ok(Value::String(self.property_key(key)?)),
        }
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
        let target = proxy.target.clone();
        let handler = proxy.handler.clone();
        let Some(trap) = self.proxy_trap(proxy, "getPrototypeOf") else {
            return self.get_prototype_of(&target);
        };
        let trap_result = self.call_this(&trap, handler, vec![target.clone()])?;
        if !matches!(trap_result, Value::Null) && !is_proxy_object(&trap_result) {
            return Err(VmErr::Msg(
                "TypeError: Proxy getPrototypeOf trap must return an object or null".into(),
            ));
        }
        if !proxy_target_is_extensible(&target) {
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

fn proxy_target_is_extensible(value: &Value) -> bool {
    match value {
        Value::Object { props } => !props.meta.borrow().non_extensible,
        Value::Array(array) => !array.meta.borrow().non_extensible,
        Value::Function(function) => !function.properties.meta.borrow().non_extensible,
        Value::Class(class) => !class.statics.meta.borrow().non_extensible,
        Value::Proxy(proxy) => proxy_target_is_extensible(&proxy.target),
        _ => false,
    }
}
