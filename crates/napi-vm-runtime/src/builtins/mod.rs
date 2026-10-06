//! Runtime globals are installed only by explicit host selection.
pub use napi_vm_core::builtins::{
    NativeFn, make_callable, read_element, set_builtin_constructor_prototype,
};
use napi_vm_core::{Interpreter, Value, VmErr, interpreter::Environment};
fn nf(name: &str, callable: NativeFn) -> Value {
    Value::NativeFunction {
        name: name.into(),
        callable,
    }
}
mod console;
mod timers;
pub use console::install_console;
pub use timers::install_timers;
#[cfg(feature = "runtime-node")]
mod buffer;
#[cfg(feature = "runtime-web")]
mod web;
#[cfg(feature = "runtime-web")]
pub fn install_web(e: &mut Environment) {
    for name in ["TextEncoder", "TextDecoder", "URLSearchParams"] {
        e.set(name, Value::object(vec![]));
    }
    web::install(e);
}
#[cfg(feature = "runtime-node")]
pub fn install_buffer(e: &mut Environment) {
    e.set("Buffer", Value::object(vec![]));
    buffer::install(e);
}
pub fn with_runtime_builtins() -> Interpreter {
    let vm = Interpreter::with_builtins();
    {
        let mut g = vm.global.borrow_mut();
        install_console(&mut g);
        install_timers(&mut g);
        #[cfg(feature = "runtime-web")]
        install_web(&mut g);
        #[cfg(feature = "runtime-node")]
        install_buffer(&mut g);
    }
    vm
}
