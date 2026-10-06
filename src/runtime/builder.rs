use super::{EngineBuilder, Runtime, RuntimeLimits, permissions::Permissions};
use crate::interpreter::CommonJsModuleLoader;
use crate::{ModuleLoader, VmErr};
use std::rc::Rc;

/// Runtime construction is explicit and only available with `runtime`.
/// `.build()` returns an OS scheduler; EngineBuilder always returns an interpreter.
#[derive(Default)]
pub struct RuntimeBuilder {
    engine: EngineBuilder,
    console: bool,
    timers: bool,
    filesystem: bool,
    environment: bool,
    permissions: Permissions,
    limits: RuntimeLimits,
    module_loader: Option<Rc<dyn ModuleLoader>>,
    #[cfg(feature = "runtime-npm")]
    npm: bool,
    #[cfg(feature = "runtime-net")]
    network: bool,
    #[cfg(feature = "runtime-web")]
    web: bool,
    #[cfg(feature = "runtime-node")]
    node: bool,
}
impl RuntimeBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn console(mut self) -> Self {
        self.console = true;
        self
    }
    pub fn timers(mut self) -> Self {
        self.timers = true;
        self
    }
    pub fn environment(mut self) -> Self {
        self.environment = true;
        self
    }
    pub fn permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = permissions;
        self
    }
    /// Install network operations with an explicit, default-denied policy.
    #[cfg(feature = "runtime-net")]
    pub fn network(mut self, permissions: Permissions) -> Self {
        self.network = true;
        self.permissions = self.permissions.merge(permissions);
        self
    }
    pub fn limits(mut self, limits: RuntimeLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn loop_budget(mut self, n: u64) -> Self {
        self.engine = self.engine.loop_budget(n);
        self
    }
    pub fn fuel_budget(mut self, n: u64) -> Self {
        self.limits.fuel = n;
        self
    }
    pub fn load_spec(mut self, name: &str, source: impl Into<String>) -> Self {
        self.engine = self.engine.load_spec(name, source);
        self
    }
    pub fn module_loader(mut self, loader: Rc<dyn ModuleLoader>) -> Self {
        self.module_loader = Some(loader);
        self
    }
    #[cfg(feature = "runtime-npm")]
    pub fn npm_packages(mut self) -> Self {
        self.npm = true;
        self
    }
    /// A supplied loader is a trusted host capability; ambient npm is never installed.
    pub fn commonjs_loader(mut self, loader: Rc<dyn CommonJsModuleLoader>) -> Self {
        self.engine = self.engine.commonjs_loader(loader);
        self
    }
    #[cfg(feature = "runtime-fs")]
    pub fn filesystem(mut self, permissions: Permissions) -> Self {
        self.filesystem = true;
        self.permissions = self.permissions.merge(permissions);
        self
    }
    /// Web wrappers and crypto. Transport requires separate network selection.
    #[cfg(feature = "runtime-web")]
    pub fn web_apis(mut self) -> Self {
        self.web = true;
        self
    }
    /// Implemented Node modules and package loading, selected explicitly.
    #[cfg(feature = "runtime-node")]
    pub fn node_compat(mut self) -> Self {
        self.node = true;
        self.npm = true;
        self
    }
    pub fn build(self) -> Result<Runtime, VmErr> {
        let mut interpreter = self.engine.build()?;
        {
            let mut global = interpreter.global.borrow_mut();
            if self.console {
                crate::builtins::install_console(&mut global);
            }
            if self.timers {
                crate::builtins::install_timers(&mut global);
            }
            #[cfg(feature = "runtime-web")]
            if self.web {
                crate::builtins::install_web(&mut global);
            }
            #[cfg(feature = "runtime-node")]
            if self.node {
                crate::builtins::install_buffer(&mut global);
            }
        }
        #[cfg(feature = "runtime-npm")]
        if self.npm {
            let loader = super::npm::NpmLoader::new(
                self.permissions.clone(),
                std::env::current_dir().map_err(|e| VmErr::Msg(e.to_string()))?,
                self.limits.file_bytes,
            )
            .map_err(|e| VmErr::Msg(e.to_string()))?;
            #[cfg(feature = "runtime-node")]
            let loader = if self.node {
                loader.node_builtins()
            } else {
                loader
            };
            let loader = Rc::new(loader);
            interpreter.set_commonjs_loader(loader.clone())?;
            interpreter.set_module_loader(loader);
        }
        if let Some(loader) = self.module_loader {
            interpreter.set_module_loader(loader);
        }
        let runtime = Runtime::from_interpreter(
            interpreter,
            self.permissions,
            self.limits,
            self.filesystem,
            self.environment,
        )?;
        #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
        let mut runtime = runtime;
        #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
        if self.network {
            runtime.install_network();
        }
        #[cfg(feature = "runtime-web")]
        let mut runtime = runtime;
        #[cfg(feature = "runtime-web")]
        if self.web {
            runtime.install_web_runtime()?;
        }
        #[cfg(feature = "runtime-node")]
        let mut runtime = runtime;
        #[cfg(feature = "runtime-node")]
        if self.node {
            super::node::install(runtime.interpreter_mut())?;
        }
        Ok(runtime)
    }
    /// Alias for callers that want to make the scheduler return type explicit.
    pub fn build_runtime(self) -> Result<Runtime, VmErr> {
        self.build()
    }
}
