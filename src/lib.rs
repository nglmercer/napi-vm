// The NAPI layer is the only Node-specific part of the crate. Gating it behind
// a feature lets the pure-Rust core (lexer, parser, interpreter, builtins,
// format) build standalone — as a plain dependency for the language server and
// GUI frontends, and for the `wasm32` target.
#[cfg(feature = "napi")]
pub mod bindings;
pub mod builtins;
pub mod convert;
pub mod error;
pub mod format;
pub mod host;
pub mod interpreter;
pub mod lang;
pub mod lexer;
#[cfg(not(target_arch = "wasm32"))]
pub mod lsp;
pub mod parser;
pub mod plugin_host;
pub mod span;
pub mod value;
// `wasm` also requires the wasm32 target: the module depends on `js-sys`, which
// is a target-scoped dependency, so `--all-features` on a native host would
// otherwise fail to compile.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm;

#[cfg(feature = "napi")]
pub use bindings::{LanguageService, VM, create_vm, debug_parse, run_code};
pub use builtins::setup_builtins;
pub use convert::{value_from_json, value_to_json};
pub use error::VmErr;
pub use format::{PrintOptions, Printer};
pub use host::{HostBridge, HostCallback, HostCallbackKind, HostEvent, WakeNotifier, WakeSlot};
pub use interpreter::{
    CommonJsModuleFormat, CommonJsModuleLoader, FileCommonJsLoader, NativeAddonLoader,
    ResolvedCommonJsModule,
};
pub use interpreter::{Environment, Interpreter, Module, PreparedProgram};
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
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use plugin_host::RustPluginNapiOptions;
pub use plugin_host::{
    DEFAULT_MAX_PLUGIN_FILE_BYTES, PLUGIN_MANIFEST_FILENAME, PluginHostError, RustLoadedPlugin,
    RustPluginCapability, RustPluginFunction, RustPluginHost, RustPluginHostOptions,
    RustPluginManifest, RustPluginPolicy, RustPluginStatus,
};
pub use value::Value;
pub mod bigint;
pub mod regex;
