//! Tier-up seam: hotness counting, JIT backends, guards, and deopt.
//!
//! The bytecode VM counts every function entry and loop back-edge. Once a
//! function trips the [`JitPolicy`] thresholds, the next entry asks the
//! registered [`JitBackend`] to compile it; compiled code runs only while
//! its [`ShapeGuard`]s hold, and any failure deoptimizes back to bytecode.
//!
//! Phase J builds the whole pipeline except the machine-code emitter: no
//! backend ships with the crate, so every tier-up decision lands on
//! bytecode. A future backend implements one trait ([`JitBackend`]) and
//! plugs into a counted, guarded, tested path — the policy, the compiled-
//! code cache, guard checking, deopt counting, and code discarding after
//! repeated deopts all exist and are exercised by a mock backend below.
//!
//! [`ShapeGuard`]: struct.ShapeGuard.html

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::bytecode::BytecodeFunction;
use crate::value::Value;

/// When a function becomes worth compiling, and when compiled code that
/// keeps deoptimizing gets thrown away.
#[derive(Debug, Clone)]
pub struct JitPolicy {
    /// Compile once a function has been entered this many times.
    pub compile_at_calls: u32,
    /// Compile once its loops have iterated this many times in total.
    pub compile_at_iters: u64,
    /// Discard compiled code after this many guard failures, so a backend
    /// can recompile against the shapes actually arriving. Zero disables
    /// discarding: guards re-check on every entry instead.
    pub max_deopts: u32,
}

impl Default for JitPolicy {
    fn default() -> Self {
        Self { compile_at_calls: 50, compile_at_iters: 5000, max_deopts: 8 }
    }
}

/// What the tier-up check observed about one function entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierDecision {
    /// Below every threshold: run bytecode, ask nothing.
    Cold,
    /// Hot, but no backend is registered.
    NoBackend,
    /// The backend looked and declined (unsupported constructs, budget).
    Declined,
    /// Compiled code exists but a guard failed: run bytecode, count a deopt.
    GuardFailed,
    /// Guards passed but the artifact has no executable payload — the only
    /// outcome a Phase J backend can produce. Runs bytecode.
    NotExecutable,
    /// Guards passed and the artifact is executable. Unreachable until a
    /// backend emits machine code; the entry path below treats it exactly
    /// like [`TierDecision::NotExecutable`] so the fallback stays tested.
    EnterJit,
}

/// One entry guard on a compiled function: argument `param` must be an
/// object (or class, or function) whose shape is `shape`. Guards name
/// shapes, never values: the same guard serves every object with the
/// layout the backend specialized for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeGuard {
    pub param: u16,
    pub shape: u32,
}

impl ShapeGuard {
    /// Whether `args` satisfies this guard. A missing argument fails, as
    /// does an argument with no cached layout: the backend specialized for
    /// a call shape the caller did not provide.
    pub fn check(&self, args: &[Value]) -> bool {
        let Some(arg) = args.get(self.param as usize) else {
            return false;
        };
        let props = match arg {
            Value::Object { props } => props,
            Value::Class(class) => &class.statics,
            Value::Function(function) => &function.properties,
            Value::HostFunction { properties, .. } => properties,
            _ => return false,
        };
        props.shape_id().is_some_and(|id| id == self.shape)
    }
}

/// Per-function tier-up feedback: what the VM observed, handed to the
/// backend so it can specialize (loop-heavy versus call-heavy, and which
/// inline-cache sites went megamorphic, once backends read them).
#[derive(Debug, Clone, Default)]
pub struct TierFeedback {
    pub calls: u32,
    pub loop_iters: u64,
}

/// One backend's compiled artifact for one function. Opaque to the VM
/// except for the guards: entry checks them, failure deoptimizes, and
/// repeated failure discards the artifact.
#[derive(Debug, Clone)]
pub struct JitCode {
    /// Which backend produced this, for diagnostics.
    pub backend: String,
    /// Entry guards; all must pass or the call runs bytecode.
    pub guards: Vec<ShapeGuard>,
    /// Whether the artifact carries runnable machine code. Always false
    /// in Phase J: backends can compile, guard, and be measured, but the
    /// VM still runs bytecode.
    pub executable: bool,
    /// Guard failures since compilation. Reaching the policy's cap
    /// discards the artifact; the next hot entry recompiles.
    pub deopts: Cell<u32>,
    /// Backend payload. Reserved for machine code; empty for now.
    pub payload: Vec<u8>,
}

impl JitCode {
    /// Whether every guard passes for these arguments.
    pub fn guards_hold(&self, args: &[Value]) -> bool {
        self.guards.iter().all(|guard| guard.check(args))
    }
}

/// A native-code backend. The VM calls [`compile`](JitBackend::compile) at
/// most once per function until the artifact is discarded: returning `None`
/// declines (the function stays on bytecode and is never asked again),
/// while `Some` caches the artifact on the function and guards every
/// later entry.
pub trait JitBackend {
    /// Backend name for diagnostics (`JitCode::backend` should match it).
    fn name(&self) -> &str;
    /// Compile `func`, specialized with `feedback`. Returning `None`
    /// declines permanently for this function.
    fn compile(&self, func: &BytecodeFunction, feedback: &TierFeedback) -> Option<JitCode>;
}

/// Hotness counters for one function, bumped by the VM.
#[derive(Debug, Clone, Default)]
pub struct TierCounters {
    /// Entries through the tier-up check.
    pub calls: Cell<u32>,
    /// Loop-head ticks (iterations plus one entry tick per loop).
    pub loop_iters: Cell<u64>,
    /// Cached compilation, if a backend produced one and it survived.
    /// `Declined` caches as `Some(None)`: asked once, never again.
    pub code: RefCell<Option<Option<JitCode>>>,
}

impl TierCounters {
    /// Snapshot for a backend's specialization decisions.
    pub fn feedback(&self) -> TierFeedback {
        TierFeedback { calls: self.calls.get(), loop_iters: self.loop_iters.get() }
    }
}

/// One backend invocation for a tier-up check: compile feedback into code.
type CompileOne<'a> = &'a dyn Fn(&TierFeedback) -> Option<JitCode>;

/// Decide how one function entry executes: count it, and past the
/// thresholds compile (once), guard, and deopt. The `compile` closure runs
/// the registered backend; passing `None` models an interpreter with no
/// backend. Every path that can execute runs bytecode — including
/// [`TierDecision::EnterJit`], which stays unreachable until a backend
/// marks an artifact executable.
pub(crate) fn tier_enter(
    counters: &TierCounters,
    policy: &JitPolicy,
    compile: Option<CompileOne<'_>>,
    args: &[Value],
) -> TierDecision {
    counters.calls.set(counters.calls.get().wrapping_add(1));
    let hot = counters.calls.get() >= policy.compile_at_calls
        || counters.loop_iters.get() >= policy.compile_at_iters;
    if !hot {
        return TierDecision::Cold;
    }
    let Some(compile) = compile else {
        return TierDecision::NoBackend;
    };
    if counters.code.borrow().is_none() {
        let feedback = counters.feedback();
        *counters.code.borrow_mut() = Some(compile(&feedback));
    }
    let cached = counters.code.borrow();
    let Some(code) = cached.as_ref().expect("compiled-or-declined just above") else {
        return TierDecision::Declined;
    };
    if !code.guards_hold(args) {
        let deopts = code.deopts.get().wrapping_add(1);
        code.deopts.set(deopts);
        if policy.max_deopts > 0 && deopts >= policy.max_deopts {
            drop(cached);
            *counters.code.borrow_mut() = None;
        }
        return TierDecision::GuardFailed;
    }
    if !code.executable {
        return TierDecision::NotExecutable;
    }
    TierDecision::EnterJit
}

/// Observe one loop back-edge for tier-up purposes.
pub(crate) fn note_loop_iter(counters: &TierCounters) {
    counters.loop_iters.set(counters.loop_iters.get().wrapping_add(1));
}

/// Shareable backend handle for interpreter configuration.
pub type BackendRef = Rc<dyn JitBackend>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that compiles everything to guarded-but-unexecutable code
    /// and counts how often it is asked.
    struct MockBackend {
        compiles: Cell<u32>,
        guards: Vec<ShapeGuard>,
    }

    impl JitBackend for MockBackend {
        fn name(&self) -> &str {
            "mock"
        }

        fn compile(&self, _func: &BytecodeFunction, _feedback: &TierFeedback) -> Option<JitCode> {
            self.compiles.set(self.compiles.get() + 1);
            Some(JitCode {
                backend: self.name().to_string(),
                guards: self.guards.clone(),
                executable: false,
                deopts: Cell::new(0),
                payload: Vec::new(),
            })
        }
    }

    struct DecliningBackend;

    impl JitBackend for DecliningBackend {
        fn name(&self) -> &str {
            "decliner"
        }

        fn compile(&self, _func: &BytecodeFunction, _feedback: &TierFeedback) -> Option<JitCode> {
            None
        }
    }

    fn policy() -> JitPolicy {
        JitPolicy { compile_at_calls: 3, compile_at_iters: 1000, max_deopts: 2 }
    }

    /// Empty function for backend calls. `tier_enter` never touches it and
    /// the mocks ignore it; the real call site passes the running function.
    fn fake_func() -> BytecodeFunction {
        BytecodeFunction {
            name: None,
            code: Vec::new(),
            constants: Vec::new(),
            register_count: 0,
            local_count: 0,
            parameter_count: 0,
            upvalue_count: 0,
            slots: Vec::new(),
            is_arrow: false,
            is_constructor: false,
            captures_arguments: false,
            caches: Vec::new().into_boxed_slice(),
            tiers: TierCounters::default(),
        }
    }

    #[test]
    fn cold_until_threshold_then_compiles_once() {
        let backend = MockBackend { compiles: Cell::new(0), guards: Vec::new() };
        let counters = TierCounters::default();
        let policy = policy();
        let run = |c: &TierCounters| {
            tier_enter(c, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[])
        };
        assert_eq!(run(&counters), TierDecision::Cold);
        assert_eq!(run(&counters), TierDecision::Cold);
        // Third call trips `compile_at_calls`: the backend compiles, the
        // empty guard set holds, and Phase J still runs bytecode.
        assert_eq!(run(&counters), TierDecision::NotExecutable);
        assert_eq!(run(&counters), TierDecision::NotExecutable);
        // Compiled once despite two hot entries.
        assert_eq!(backend.compiles.get(), 1);
    }

    #[test]
    fn no_backend_reports_hot() {
        let counters = TierCounters::default();
        let policy = policy();
        assert_eq!(tier_enter(&counters, &policy, None, &[]), TierDecision::Cold);
        assert_eq!(tier_enter(&counters, &policy, None, &[]), TierDecision::Cold);
        for _ in 0..3 {
            assert_eq!(tier_enter(&counters, &policy, None, &[]), TierDecision::NoBackend);
        }
    }

    #[test]
    fn decline_asks_once() {
        let backend = DecliningBackend;
        let counters = TierCounters::default();
        let policy = policy();
        assert_eq!(
            tier_enter(&counters, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[]),
            TierDecision::Cold
        );
        assert_eq!(
            tier_enter(&counters, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[]),
            TierDecision::Cold
        );
        assert_eq!(
            tier_enter(&counters, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[]),
            TierDecision::Declined
        );
        assert_eq!(
            tier_enter(&counters, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[]),
            TierDecision::Declined
        );
    }

    #[test]
    fn guard_failure_deopts_then_discards() {
        let backend = MockBackend {
            compiles: Cell::new(0),
            // Param 0 must have shape 424242: never true for `[]`.
            guards: vec![ShapeGuard { param: 0, shape: 424242 }],
        };
        let counters = TierCounters::default();
        let policy = policy();
        let run =
            |c: &TierCounters| tier_enter(c, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[]);
        assert_eq!(run(&counters), TierDecision::Cold);
        assert_eq!(run(&counters), TierDecision::Cold);
        assert_eq!(run(&counters), TierDecision::GuardFailed);
        // Second failure hits `max_deopts`: discarded, recompiled next.
        assert_eq!(run(&counters), TierDecision::GuardFailed);
        assert_eq!(backend.compiles.get(), 1);
        assert_eq!(run(&counters), TierDecision::GuardFailed);
        assert_eq!(backend.compiles.get(), 2);
    }

    #[test]
    fn loop_iters_trip_threshold() {
        let backend = MockBackend { compiles: Cell::new(0), guards: Vec::new() };
        let counters = TierCounters::default();
        let policy = policy();
        for _ in 0..1000 {
            note_loop_iter(&counters);
        }
        // First call is already hot through the loop counter.
        assert_eq!(
            tier_enter(&counters, &policy, Some(&|f| backend.compile(&fake_func(), f)), &[]),
            TierDecision::NotExecutable
        );
        assert_eq!(backend.compiles.get(), 1);
    }

    #[test]
    fn hot_function_requests_compile_through_vm() {
        use crate::bytecode::{compile_program, verify_module};
        use crate::interpreter::Interpreter;
        use crate::parser::parse_cached;
        let src = "function f(x) { return x + 1; } let s = 0; \
                   for (let i = 0; i < 10; i++) { s = s + f(i); } s;";
        let statements = parse_cached(src).expect("test source must parse");
        let module = compile_program(&statements).expect("test must reach the bytecode tier");
        verify_module(&module).expect("compiler output must verify");
        let backend = Rc::new(MockBackend { compiles: Cell::new(0), guards: Vec::new() });
        let mut interp = Interpreter::with_builtins();
        interp.set_jit_backend(backend.clone());
        interp.set_jit_policy(JitPolicy {
            compile_at_calls: 3,
            compile_at_iters: 1_000_000,
            max_deopts: 8,
        });
        interp.begin_execution();
        interp.set_source(src);
        let result = interp.run_bytecode_module(&module).expect("must run");
        // f(0) + ... + f(9) = 1 + ... + 10, on bytecode throughout.
        assert!(matches!(result, Value::Number(n) if n == 55.0), "got {result:?}");
        // Ten entries, one compilation: the seam asked, cached, and fell
        // back to bytecode every time.
        assert_eq!(backend.compiles.get(), 1);
    }

    #[test]
    fn shape_guard_checks_params() {
        let guarded = ShapeGuard { param: 1, shape: 7 };
        // Missing argument fails.
        assert!(!guarded.check(&[Value::Undefined]));
        // Non-object fails.
        assert!(!guarded.check(&[Value::Undefined, Value::Number(1.0)]));
        // The guard compares shape ids; any object disagrees with 7 unless
        // the thread-local counter happens to align, so assert the shape
        // plumbing instead of a fixed outcome.
        let obj = Value::object(vec![("a".to_string(), Value::Number(1.0))]);
        let Value::Object { props } = &obj else { unreachable!() };
        // Unbuilt objects fail every guard; two reads build the layout.
        assert!(!ShapeGuard { param: 0, shape: 0 }.check(std::slice::from_ref(&obj)));
        props.own_index("a");
        props.own_index("a");
        let id = props.shape_id().expect("two reads build the layout");
        let matching = ShapeGuard { param: 0, shape: id };
        assert!(matching.check(std::slice::from_ref(&obj)));
        let other = ShapeGuard { param: 0, shape: id.wrapping_add(1 << 20) };
        assert!(!other.check(std::slice::from_ref(&obj)));
    }
}
