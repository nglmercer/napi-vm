//! Unified selection for native addon hosts.
//!
//! The backend-specific option types remain available so hosts can configure
//! the Node executable or the in-process Node-API surface without losing
//! backend-specific controls. This enum gives embedders one entry point for
//! installing either backend on an interpreter.

use std::rc::Rc;

use super::node_addon::{NodeAddonOptions, NodeAddonSidecar};
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
use super::rust_node_api::{RustNodeApiHost, RustNodeApiOptions};

/// Configuration for one native addon backend.
///
/// Pass the existing backend-specific options to
/// [`Interpreter::enable_native_addons`](super::Interpreter::enable_native_addons).
/// Native loading remains disabled until an explicit addon allowlist is
/// provided through the selected options type.
#[derive(Clone, Debug)]
pub enum NativeAddonOptions {
    /// Run addons inside a configured Node.js child process.
    NodeSidecar(NodeAddonOptions),
    /// Load Node-API addons directly into the Rust host process.
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    RustNodeApi(RustNodeApiOptions),
}

impl From<NodeAddonOptions> for NativeAddonOptions {
    fn from(options: NodeAddonOptions) -> Self {
        Self::NodeSidecar(options)
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl From<RustNodeApiOptions> for NativeAddonOptions {
    fn from(options: RustNodeApiOptions) -> Self {
        Self::RustNodeApi(options)
    }
}

/// A configured native addon host retained by an interpreter.
///
/// The interpreter also retains the host internally. This value lets an
/// embedding application inspect the selected backend and, for the sidecar,
/// its reported Node and Node-API versions.
#[derive(Clone)]
pub enum NativeAddonRuntime {
    /// Node.js child process backend.
    NodeSidecar(Rc<NodeAddonSidecar>),
    /// In-process Node-API backend.
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    RustNodeApi(Rc<RustNodeApiHost>),
}

impl NativeAddonRuntime {
    /// Name of the backend selected for this interpreter.
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::NodeSidecar(_) => "node-sidecar",
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            Self::RustNodeApi(_) => "rust-node-api",
        }
    }

    /// Return the Node sidecar when that backend is selected.
    pub fn node_sidecar(&self) -> Option<&NodeAddonSidecar> {
        match self {
            Self::NodeSidecar(sidecar) => Some(sidecar),
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            Self::RustNodeApi(_) => None,
        }
    }

    /// Return the in-process Node-API host when that backend is selected.
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    pub fn rust_node_api(&self) -> Option<&RustNodeApiHost> {
        match self {
            Self::NodeSidecar(_) => None,
            Self::RustNodeApi(host) => Some(host),
        }
    }
}
