//! Runtime construction and observability.
//!
//! [`RuntimeBuilder`] configures an interpreter in one place — budgets,
//! the JIT backend and policy, module specs, loaders — instead of a
//! sequence of setter calls at every embedding site. [`RuntimeStats`]
//! snapshots the thread-global counters (heap, shapes) for monitoring.

use std::path::Path;
use std::rc::Rc;

use crate::error::VmErr;
use crate::host::HostBridge;
use crate::interpreter::{CommonJsModuleLoader, Interpreter};

/// Thread-global runtime counters: the managed heap and the shape tree.
/// Per-function counters (tier hotness, inline-cache hits) live on the
/// functions themselves — see [`BytecodeFunction::stats`] and
/// [`PreparedProgram::stats`].
///
/// [`BytecodeFunction::stats`]: crate::bytecode::BytecodeFunction::stats
/// [`PreparedProgram::stats`]: crate::interpreter::PreparedProgram::stats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeStats {
    /// Heap containers currently tracked for cycle collection.
    pub heap_tracked: usize,
    /// Cycles reclaimed over the thread's lifetime.
    pub heap_collected_total: u64,
    /// Object shapes minted on this thread (monotonic; wraps only past
    /// four billion). Deltas across a workload measure layout churn.
    pub shapes_created: u32,
}

/// Build a configured interpreter: budgets, JIT backend and policy, host
/// bridge, module loaders, and preloaded module specs.
///
/// ```rust
/// use napi_vm::runtime::RuntimeBuilder;
///
/// let mut interp = RuntimeBuilder::new()
///     .loop_budget(10_000)
///     .load_spec("constants", "export const ANSWER = 42;")
///     .build()
///     .expect("runtime builds");
/// let answer = interp.eval_source("import { ANSWER } from 'constants'; ANSWER;");
/// assert!(matches!(answer, Ok(napi_vm::Value::Number(n)) if n == 42.0));
/// ```
#[derive(Default)]
pub struct RuntimeBuilder {
    loop_budget: Option<u64>,
    fuel_budget: Option<u64>,
    jit_backend: Option<crate::jit::BackendRef>,
    jit_policy: Option<crate::jit::JitPolicy>,
    host_bridge: Option<Rc<dyn HostBridge>>,
    commonjs_loader: Option<Rc<dyn CommonJsModuleLoader>>,
    specs: Vec<(String, String)>,
}

impl RuntimeBuilder {
    /// Start from interpreter defaults with no specs loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cap loop iterations per execution (also refilled to it).
    pub fn loop_budget(mut self, n: u64) -> Self {
        self.loop_budget = Some(n);
        self
    }

    /// Cap bytecode instruction fuel per execution (also refilled to it).
    pub fn fuel_budget(mut self, n: u64) -> Self {
        self.fuel_budget = Some(n);
        self
    }

    /// Register a native-code backend for the JIT seam.
    pub fn jit_backend(mut self, backend: crate::jit::BackendRef) -> Self {
        self.jit_backend = Some(backend);
        self
    }

    /// Tune when functions tier up and when deoptimizing code is discarded.
    pub fn jit_policy(mut self, policy: crate::jit::JitPolicy) -> Self {
        self.jit_policy = Some(policy);
        self
    }

    /// Install the host bridge for calls into Node.js.
    pub fn host_bridge(mut self, bridge: Rc<dyn HostBridge>) -> Self {
        self.host_bridge = Some(bridge);
        self
    }

    /// Install a CommonJS module loader (provides `require`).
    pub fn commonjs_loader(mut self, loader: Rc<dyn CommonJsModuleLoader>) -> Self {
        self.commonjs_loader = Some(loader);
        self
    }

    /// Preload a module spec: `name` becomes importable with `source` as
    /// its body, exactly as if [`Interpreter::define_module`] had been
    /// called after construction. Sources stay lazy — they parse when
    /// first imported, so a spec with a syntax error fails at import,
    /// not here.
    pub fn load_spec(mut self, name: &str, source: impl Into<String>) -> Self {
        self.specs.push((name.to_string(), source.into()));
        self
    }

    /// Preload a module spec from a file, deriving the spec name from the
    /// file stem (`"./lib/constants.js"` becomes `"constants"`). Reads
    /// immediately, so a missing file or a path without a stem errors now.
    pub fn load_spec_file(mut self, path: &Path) -> std::io::Result<Self> {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("cannot derive a spec name from {}", path.display()),
                )
            })?;
        let source = std::fs::read_to_string(path)?;
        self.specs.push((name.to_string(), source));
        Ok(self)
    }

    /// Construct the interpreter with builtins installed, then apply every
    /// configured option and preload every spec. Only loader installation
    /// can fail (it wires `require` into the fresh global scope).
    pub fn build(self) -> Result<Interpreter, VmErr> {
        let mut interp = Interpreter::with_builtins();
        if let Some(n) = self.loop_budget {
            interp.set_loop_budget(n);
        }
        if let Some(n) = self.fuel_budget {
            interp.set_fuel_budget(n);
        }
        if let Some(backend) = self.jit_backend {
            interp.set_jit_backend(backend);
        }
        if let Some(policy) = self.jit_policy {
            interp.set_jit_policy(policy);
        }
        if let Some(bridge) = self.host_bridge {
            interp.set_host_bridge(bridge);
        }
        if let Some(loader) = self.commonjs_loader {
            interp.set_commonjs_loader(loader)?;
        }
        for (name, source) in self.specs {
            interp.define_module(&name, source);
        }
        Ok(interp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    #[test]
    fn spec_is_importable_after_build() {
        let mut interp =
            RuntimeBuilder::new().load_spec("constants", "export const ANSWER = 42;").build().unwrap();
        let answer = interp.eval_source("import { ANSWER } from 'constants'; ANSWER;").unwrap();
        assert!(matches!(answer, Value::Number(n) if n == 42.0), "got {answer:?}");
    }

    #[test]
    fn loop_budget_applies() {
        let mut interp = RuntimeBuilder::new().loop_budget(5).build().unwrap();
        let result = interp.eval_source("let s = 0; for (let i = 0; i < 100; i++) { s += i; } s;");
        assert!(matches!(&result, Err(VmErr::Msg(m)) if m.contains("loop")), "got {result:?}");
    }

    #[test]
    fn missing_spec_file_errors() {
        let result = RuntimeBuilder::new().load_spec_file(Path::new("/nonexistent-dir-7f3a/spec.js"));
        assert!(result.is_err());
    }

    #[test]
    fn spec_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("napi-vm-spec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("greeting.js");
        std::fs::write(&path, "export const WORD = 'hi';").unwrap();
        let mut interp =
            RuntimeBuilder::new().load_spec_file(&path).unwrap().build().unwrap();
        let word = interp.eval_source("import { WORD } from 'greeting'; WORD;").unwrap();
        assert!(matches!(&word, Value::String(s) if s == "hi"), "got {word:?}");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn prepared_program_stats() {
        use crate::interpreter::Interpreter;
        let program =
            Interpreter::compile("let s = 0; for (let i = 0; i < 5; i++) { s += i; } s;").unwrap();
        let mut interp = RuntimeBuilder::new().build().unwrap();
        interp.execute(&program).unwrap();
        let stats = program.stats().expect("loop program must reach the bytecode tier");
        assert_eq!(stats.calls, 1);
        // Five iterations plus the loop-entry tick.
        assert_eq!(stats.loop_iters, 6);
    }

    #[test]
    fn runtime_stats_snapshot() {
        let interp = RuntimeBuilder::new().build().unwrap();
        let stats = interp.runtime_stats();
        // A fresh runtime tracks hundreds of builtin containers and has
        // already minted shapes for them; exact counts don't matter.
        assert!(stats.heap_tracked > 0, "got {stats:?}");
        assert!(stats.shapes_created > 0, "got {stats:?}");
    }
}
