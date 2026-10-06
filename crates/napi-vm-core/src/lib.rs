//! Embeddable JavaScript engine with no automatically installed runtime capabilities.
//! Host adapters and optional transports are provided by separate crates.
pub mod js_string;
pub use js_string::JsString;
pub mod builtins;
pub mod bytecode;
pub mod convert;
pub mod error;
pub mod format;
pub mod heap;
pub mod host;
pub mod interpreter;
pub mod jit;
pub mod lang;
pub mod lexer;
pub mod module_loader;
pub use module_loader::{
    CompositeLoader, DataUrlLoader, ModuleLoader, ModuleSource, VirtualLoader,
};
pub mod parser;
pub mod runtime;
pub mod shape;
pub mod span;
pub mod value;
pub use builtins::setup_builtins;
pub use convert::{value_from_json, value_to_json};
pub use error::VmErr;
pub use format::{PrintOptions, Printer};
pub use host::{
    HostBridge, HostCallback, HostCallbackKind, HostEvent, WakeNotifier, WakeSignal, WakeSlot,
};
pub use interpreter::{
    CancellationToken, Clock, ClockMode, Environment, EventLoopOptions, Fairness, Interpreter,
    Module, PreparedProgram, RealTimeClock, TurnBudget, TurnOutcome, VirtualClock, YieldReason,
};
pub use interpreter::{
    CommonJsModuleFormat, CommonJsModuleLoader, FileCommonJsLoader, NativeAddonLoader,
    ResolvedCommonJsModule,
};
#[cfg(not(target_arch = "wasm32"))]
pub use interpreter::{
    NativeAddonBackendHost, NativeAddonOptions, NativeAddonPolicy, NativeAddonRuntime,
    NodeAddonOptions, NodeAddonRuntimeInfo, NodeAddonSidecar,
};
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use interpreter::{ReportedNodeVersion, RustNodeApiHost, RustNodeApiOptions};
pub use lexer::{Lexer, Token};
pub use parser::{Expr, Parser, Statement};
pub use value::Value;
pub mod bigint;
pub mod regex;

#[cfg(test)]
mod test_support;
