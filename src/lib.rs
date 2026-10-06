//! Binding and CLI facade. Engine and runtime implementations live in separate crates.
pub use napi_vm_core::*;
#[cfg(feature = "napi")]
pub mod bindings;
#[cfg(not(target_arch = "wasm32"))]
pub mod lsp;
pub mod plugin_host;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm;
pub mod runtime {
    pub use napi_vm_core::runtime::*;
    #[cfg(feature = "runtime")]
    pub use napi_vm_runtime::runtime::*;
}
#[cfg(feature = "napi")]
pub use bindings::{
    AsyncSession, AsyncSessionOptions, LanguageService, RuntimeCapabilities, VM, create_vm,
    debug_parse, run_code,
};
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
