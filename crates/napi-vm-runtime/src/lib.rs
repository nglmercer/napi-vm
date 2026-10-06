//! Explicit, optional runtime capabilities for napi-vm-core.
pub use napi_vm_core::*;
pub mod builtins;
pub mod runtime;
pub use runtime::*;

#[cfg(feature = "runtime-cli")]
pub mod cli;
