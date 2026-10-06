#[cfg(feature = "runtime")]
mod builder;
#[cfg(feature = "runtime")]
mod optional;
#[cfg(feature = "runtime")]
pub use builder::RuntimeBuilder;
#[cfg(feature = "runtime")]
pub use optional::{ExternalEventQueue, ExternalEventSender, Runtime, RuntimeLimits};
#[cfg(any(feature = "runtime-fs", feature = "runtime-net"))]
pub mod loaders;
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
mod network;
#[cfg(feature = "runtime-node")]
pub mod node;
#[cfg(feature = "runtime-npm")]
pub mod npm;
#[cfg(feature = "runtime")]
pub mod permissions;
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
mod sockets;
#[cfg(feature = "runtime-typescript")]
pub mod typescript;
#[cfg(feature = "runtime-web")]
mod web;

pub use napi_vm_core::runtime::{EngineBuilder, RuntimeStats};

#[cfg(feature = "runtime-node")]
pub use crate::builtins::install_buffer;
#[cfg(feature = "runtime-web")]
pub use crate::builtins::install_web;
pub use crate::builtins::{install_console, install_timers, with_runtime_builtins};
