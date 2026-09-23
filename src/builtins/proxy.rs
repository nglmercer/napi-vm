//! `Proxy`: a target object wrapped by a handler whose traps intercept the
//! fundamental operations.
//!
//! Only the traps this interpreter can route are supported — `get`, `set`,
//! `has`, `deleteProperty`, `ownKeys`, `apply` and `construct`. An operation
//! with no trap falls through to the target, which is what makes an empty
//! handler transparent.

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
}
