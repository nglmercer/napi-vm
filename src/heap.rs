//! Managed heap: allocation registry with mark-sweep cycle collection.
//!
//! Guest values are reference-counted, so cycles — an environment holding a
//! function that closes over it, an object referencing itself, a promise
//! reaction loop — never free on their own. Every plugin reload and every
//! long-lived script leaks them. This module tracks every heap container at
//! creation and reclaims unreachable cycles on demand.
//!
//! Collection runs only at quiescent points: [`Interpreter::collect_cycles`]
//! refuses while any interpreter on this thread is executing, and while a
//! generator or async task is suspended (its coroutine stack holds values
//! the tracer cannot see). Roots are the union of every live interpreter's
//! roots — globals, module records, job queues — plus host-pinned values,
//! so one thread's interpreters never collect each other's reachable state.
//!
//! [`Value`]: crate::value::Value
//! [`Interpreter::collect_cycles`]: crate::interpreter::Interpreter::collect_cycles

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

#[cfg(stackful_coroutines)]
use crate::interpreter::AsyncTask;
use crate::interpreter::Env;
use crate::interpreter::Environment;
use crate::value::{
    ArrayCell, ClassData, FunctionData, GeneratorInner, ObjectCell, PromiseInner, ProxyData, Value,
};

/// Identity of a tracked heap object: the address of its `Rc` block. Unique
/// among live allocations; dead registry entries prune on every collection.
type HeapId = usize;

fn id_of<T>(rc: &Rc<T>) -> HeapId {
    Rc::as_ptr(rc) as usize
}

/// Live GC roots of one interpreter: values plus the environments, module
/// scopes, and queues that own them. Handles stay live, so a walk always
/// sees current contents no matter when the interpreter last ran.
#[derive(Clone, Default)]
pub(crate) struct GcRoots {
    pub envs: Vec<Env>,
    pub values: Vec<Value>,
}

/// Per-collection statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeapStats {
    /// Registry entries (live or dead) when collection started.
    pub tracked: usize,
    /// Objects reached from the roots.
    pub marked: usize,
    /// Unreachable cycles reclaimed.
    pub collected: usize,
    /// Why collection refused to run, if it did.
    pub skipped: Option<SkipReason>,
}

/// Why a collection pass did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Guest code is executing on this thread: Rust-stack temporaries hold
    /// values the root set cannot see.
    Executing,
    /// A generator or async task is suspended: its coroutine stack holds
    /// values the tracer cannot see.
    SuspendedTasks,
}

struct HeapInner {
    objects: Vec<Weak<ObjectCell>>,
    arrays: Vec<Weak<ArrayCell>>,
    envs: Vec<Weak<RefCell<Environment>>>,
    functions: Vec<Weak<FunctionData>>,
    promises: Vec<Weak<RefCell<PromiseInner>>>,
    generators: Vec<Weak<RefCell<GeneratorInner>>>,
    proxies: Vec<Weak<ProxyData>>,
    bindings: Vec<Weak<RefCell<Value>>>,
    #[cfg(stackful_coroutines)]
    async_tasks: Vec<Weak<RefCell<AsyncTask>>>,
    /// Live interpreters: roots plus the execution-depth handle that gates
    /// collection. Keyed by construction id; moves are harmless because
    /// nothing here points into an interpreter.
    interps: HashMap<u64, (GcRoots, std::rc::Rc<std::cell::Cell<usize>>)>,
    next_interp: u64,
    pins: HashMap<u64, Value>,
    next_pin: u64,
    total_collected: u64,
}

impl HeapInner {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
            arrays: Vec::new(),
            envs: Vec::new(),
            functions: Vec::new(),
            promises: Vec::new(),
            generators: Vec::new(),
            proxies: Vec::new(),
            bindings: Vec::new(),
            #[cfg(stackful_coroutines)]
            async_tasks: Vec::new(),
            interps: HashMap::new(),
            next_interp: 1,
            pins: HashMap::new(),
            next_pin: 1,
            total_collected: 0,
        }
    }

    fn tracked_count(&self) -> usize {
        self.objects.len()
            + self.arrays.len()
            + self.envs.len()
            + self.functions.len()
            + self.promises.len()
            + self.generators.len()
            + self.proxies.len()
            + self.bindings.len()
            + {
                #[cfg(stackful_coroutines)]
                {
                    self.async_tasks.len()
                }
                #[cfg(not(stackful_coroutines))]
                {
                    0
                }
            }
    }
}

thread_local! {
    static HEAP: RefCell<HeapInner> = RefCell::new(HeapInner::new());
}

/// A heap container that registers itself at creation. Missed mutable
/// sites leak (the object is never swept) but stay sound: the marker
/// reaches them through their parents' edges either way. Immutable payloads
/// (`FunctionData`, `ProxyData`) are deliberately never registered — every
/// cycle through one also passes a mutable container whose clearing breaks
/// it — so their registries exist only to keep the sweep uniform.
/// `AsyncTask` *is* registered, but only so the suspend barrier can find
/// suspended tasks; its cycles break through the result promise.
pub trait Track {
    /// Register `rc` for cycle collection.
    fn track_rc(rc: &Rc<Self>);
}

/// Wrap a fresh heap allocation so the collector tracks it.
pub fn tracked<T: Track>(rc: Rc<T>) -> Rc<T> {
    T::track_rc(&rc);
    rc
}

/// Capture an environment into a closure: the only way an `Env` becomes
/// shared. Uncaptured frame envs die by refcount and are never tracked;
/// capturing registers the env so closure cycles can be broken. Capturing
/// the same env twice registers two entries; the sweep cascade frees the
/// env on the first and prunes the second, so duplicates only cost a word.
pub(crate) fn capture_env(env: &Env) -> Env {
    tracked(env.clone())
}

macro_rules! track_impl {
    ($ty:ty, $vec:ident) => {
        impl Track for $ty {
            fn track_rc(rc: &Rc<Self>) {
                HEAP.with(|heap| heap.borrow_mut().$vec.push(Rc::downgrade(rc)));
            }
        }
    };
}

track_impl!(ObjectCell, objects);
track_impl!(ArrayCell, arrays);
track_impl!(RefCell<Environment>, envs);
track_impl!(FunctionData, functions);
track_impl!(RefCell<PromiseInner>, promises);
track_impl!(RefCell<GeneratorInner>, generators);
track_impl!(ProxyData, proxies);
track_impl!(RefCell<Value>, bindings);
#[cfg(stackful_coroutines)]
track_impl!(RefCell<AsyncTask>, async_tasks);

/// A host-owned root handle: the value (and everything it reaches) survives
/// collection until the id is removed.
pub type RootId = u64;
/// A registered interpreter's id, for root-set removal on drop.
pub(crate) type InterpId = u64;

/// One traced heap node held alive across the mark pass.
enum MarkItem {
    Object(Rc<ObjectCell>),
    Array(Rc<ArrayCell>),
    Env(Env),
    Function(Rc<FunctionData>),
    Promise(Rc<RefCell<PromiseInner>>),
    Generator(Rc<RefCell<GeneratorInner>>),
    Proxy(Rc<ProxyData>),
    Binding(Rc<RefCell<Value>>),
    #[cfg(stackful_coroutines)]
    AsyncTask(Rc<RefCell<AsyncTask>>),
}

struct Marker {
    marked: HashSet<HeapId>,
    work: Vec<MarkItem>,
}

impl Marker {
    fn new() -> Self {
        Self {
            marked: HashSet::new(),
            work: Vec::new(),
        }
    }

    fn mark_env(&mut self, env: &Env) {
        if self.marked.insert(id_of(env)) {
            self.work.push(MarkItem::Env(env.clone()));
        }
    }

    fn mark_value(&mut self, value: &Value) {
        match value {
            Value::Object { props } => {
                if self.marked.insert(id_of(props)) {
                    self.work.push(MarkItem::Object(props.clone()));
                }
            }
            Value::Array(cell) => {
                if self.marked.insert(id_of(cell)) {
                    self.work.push(MarkItem::Array(cell.clone()));
                }
            }
            Value::Function(fd) => {
                if self.marked.insert(id_of(fd)) {
                    self.work.push(MarkItem::Function(fd.clone()));
                }
            }
            Value::HostFunction { properties, .. } => {
                if self.marked.insert(id_of(properties)) {
                    self.work.push(MarkItem::Object(properties.clone()));
                }
            }
            // Boxed, so identity-less: trace through to the shared cells.
            Value::Class(cd) => self.mark_class(cd),
            Value::Promise(inner) => {
                if self.marked.insert(id_of(inner)) {
                    self.work.push(MarkItem::Promise(inner.clone()));
                }
            }
            Value::Generator { inner } => {
                if self.marked.insert(id_of(inner)) {
                    self.work.push(MarkItem::Generator(inner.clone()));
                }
            }
            Value::Proxy(data) => {
                if self.marked.insert(id_of(data)) {
                    self.work.push(MarkItem::Proxy(data.clone()));
                }
            }
            Value::Binding(cell) => {
                if self.marked.insert(id_of(cell)) {
                    self.work.push(MarkItem::Binding(cell.clone()));
                }
            }
            #[cfg(stackful_coroutines)]
            Value::AsyncTask(inner) if self.marked.insert(id_of(inner)) => {
                self.work.push(MarkItem::AsyncTask(inner.clone()));
            }
            _ => {}
        }
    }

    fn mark_class(&mut self, cd: &ClassData) {
        self.mark_value(&cd.constructor);
        self.mark_value(&cd.prototype);
        if self.marked.insert(id_of(&cd.statics)) {
            self.work.push(MarkItem::Object(cd.statics.clone()));
        }
    }

    /// Drain the worklist iteratively: deep chains never touch the Rust stack.
    fn drain(&mut self) {
        while let Some(item) = self.work.pop() {
            match item {
                MarkItem::Object(cell) => {
                    for child in cell.trace_children() {
                        self.mark_value(&child);
                    }
                }
                MarkItem::Array(cell) => {
                    for child in cell.trace_children() {
                        self.mark_value(&child);
                    }
                }
                MarkItem::Env(env) => {
                    let Ok(borrowed) = env.try_borrow() else {
                        continue;
                    };
                    for child in borrowed.trace_values() {
                        self.mark_value(&child);
                    }
                    if let Some(parent) = borrowed.trace_parent() {
                        self.mark_env(&parent);
                    }
                }
                MarkItem::Function(fd) => {
                    if self.marked.insert(id_of(&fd.properties)) {
                        self.work.push(MarkItem::Object(fd.properties.clone()));
                    }
                    if let Some(closure) = &fd.closure {
                        self.mark_env(closure);
                    }
                    if let Some(bound) = &fd.bound {
                        self.mark_value(&bound.target);
                        self.mark_value(&bound.this_value);
                        for arg in bound.arguments.iter() {
                            self.mark_value(arg);
                        }
                    }
                    // `bytecode` is code, not data: constants hold no values.
                }
                MarkItem::Promise(inner) => {
                    let Ok(borrowed) = inner.try_borrow() else {
                        continue;
                    };
                    self.mark_value(&borrowed.value);
                    for reaction in &borrowed.reactions {
                        self.mark_value(&reaction.on_fulfilled);
                        self.mark_value(&reaction.on_rejected);
                        if self.marked.insert(id_of(&reaction.derived)) {
                            self.work.push(MarkItem::Promise(reaction.derived.clone()));
                        }
                    }
                }
                MarkItem::Generator(inner) => {
                    let Ok(borrowed) = inner.try_borrow() else {
                        continue;
                    };
                    if let Some(closure) = &borrowed.closure {
                        self.mark_env(closure);
                    }
                    for arg in &borrowed.args {
                        self.mark_value(arg);
                    }
                    if let Some(value) = &borrowed.return_value {
                        self.mark_value(value);
                    }
                    #[cfg(not(stackful_coroutines))]
                    for value in borrowed.buffered.iter() {
                        self.mark_value(value);
                    }
                    // A suspended coroutine's stack is opaque by design;
                    // collection refuses to run while one is suspended.
                }
                MarkItem::Proxy(data) => {
                    self.mark_value(&data.target);
                    self.mark_value(&data.handler);
                }
                MarkItem::Binding(cell) => {
                    let Ok(borrowed) = cell.try_borrow() else {
                        continue;
                    };
                    self.mark_value(&borrowed);
                }
                #[cfg(stackful_coroutines)]
                MarkItem::AsyncTask(inner) => {
                    // The result promise is tracked in its own right; the
                    // coroutine stack is opaque, hence the suspend barrier.
                    let _ = inner;
                }
            }
        }
    }
}

/// Register an interpreter's live roots. Called once per construction; the
/// returned id unregisters on drop.
pub(crate) fn register_interp(roots: GcRoots, executing: Rc<std::cell::Cell<usize>>) -> InterpId {
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        let id = heap.next_interp;
        heap.next_interp += 1;
        heap.interps.insert(id, (roots, executing));
        id
    })
}

pub(crate) fn unregister_interp(id: InterpId) {
    HEAP.with(|heap| {
        heap.borrow_mut().interps.remove(&id);
    });
}

/// Replace an interpreter's roots, keeping its execution-depth handle.
/// Interpreters republish whenever their root handles change (setup) and
/// whenever guest execution quiesces (loop scopes strand `global` on a
/// transient child), so collection always walks current state.
pub(crate) fn republish_roots(id: InterpId, roots: GcRoots) {
    HEAP.with(|heap| {
        if let Some(slot) = heap.borrow_mut().interps.get_mut(&id) {
            slot.0 = roots;
        }
    });
}

/// Pin a host-held value across collections. The host must pin every value
/// it retains outside the interpreter's own roots — a plugin instance, a
/// cached callback — or collection may reclaim what it reaches.
pub fn add_root(value: Value) -> RootId {
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        let id = heap.next_pin;
        heap.next_pin += 1;
        heap.pins.insert(id, value);
        id
    })
}

pub fn remove_root(id: RootId) {
    HEAP.with(|heap| {
        heap.borrow_mut().pins.remove(&id);
    });
}

/// Sweep one registry: drop dead entries, reclaim unmarked live ones.
fn sweep_vec<T>(
    entries: &mut Vec<Weak<T>>,
    marked: &HashSet<HeapId>,
    clear: impl Fn(&Rc<T>) -> bool,
) -> usize {
    let mut collected = 0;
    entries.retain(|weak| {
        let Some(strong) = weak.upgrade() else {
            return false;
        };
        if marked.contains(&(Rc::as_ptr(&strong) as usize)) {
            return true;
        }
        if clear(&strong) {
            collected += 1;
            false
        } else {
            // Borrow conflict: keep the entry and try next time.
            true
        }
    });
    collected
}

/// Run one mark-sweep pass over the thread's heap.
///
/// The suspended-tasks barrier runs first: a suspended body keeps its
/// execution guard alive on the suspended stack, so suspension always
/// also reads as executing. Reporting the suspension is what tells the
/// host the actionable truth — finish or close the generator — instead of
/// claiming guest code is running when the driver is quiescent.
pub(crate) fn collect() -> HeapStats {
    let suspended = HEAP.with(|heap| {
        let heap = heap.borrow();
        heap.generators.iter().any(|weak| {
            weak.upgrade().is_some_and(|strong| {
                strong
                    .try_borrow()
                    .is_ok_and(|inner| inner.suspends_values())
            })
        }) || {
            #[cfg(stackful_coroutines)]
            {
                heap.async_tasks.iter().any(|weak| {
                    weak.upgrade().is_some_and(|strong| {
                        strong.try_borrow().is_ok_and(|task| task.suspends_values())
                    })
                })
            }
            #[cfg(not(stackful_coroutines))]
            {
                false
            }
        }
    });
    if suspended {
        return HeapStats {
            skipped: Some(SkipReason::SuspendedTasks),
            ..HeapStats::default()
        };
    }
    let executing = HEAP.with(|heap| {
        heap.borrow()
            .interps
            .values()
            .any(|(_, depth)| depth.get() > 0)
    });
    if executing {
        return HeapStats {
            skipped: Some(SkipReason::Executing),
            ..HeapStats::default()
        };
    }

    let mut marker = Marker::new();
    HEAP.with(|heap| {
        let heap = heap.borrow();
        for (roots, _) in heap.interps.values() {
            for env in &roots.envs {
                marker.mark_env(env);
            }
            for value in &roots.values {
                marker.mark_value(value);
            }
        }
        for pinned in heap.pins.values() {
            marker.mark_value(pinned);
        }
    });
    marker.drain();

    let mut stats = HeapStats {
        marked: marker.marked.len(),
        ..HeapStats::default()
    };
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        stats.tracked = heap.tracked_count();
        stats.collected += sweep_vec(&mut heap.objects, &marker.marked, |cell| cell.clear_edges());
        stats.collected += sweep_vec(&mut heap.arrays, &marker.marked, |cell| cell.clear_edges());
        stats.collected += sweep_vec(&mut heap.envs, &marker.marked, |env| {
            let Ok(mut borrowed) = env.try_borrow_mut() else {
                return false;
            };
            borrowed.clear_edges();
            true
        });
        // Immutable payloads break on the mutable side: clearing the
        // containers that reference them cascades through these nodes.
        stats.collected += sweep_vec(&mut heap.functions, &marker.marked, |_| true);
        stats.collected += sweep_vec(&mut heap.promises, &marker.marked, |inner| {
            let Ok(mut borrowed) = inner.try_borrow_mut() else {
                return false;
            };
            borrowed.value = Value::Undefined;
            borrowed.reactions.clear();
            true
        });
        stats.collected += sweep_vec(&mut heap.generators, &marker.marked, |inner| {
            let Ok(mut borrowed) = inner.try_borrow_mut() else {
                return false;
            };
            borrowed.args.clear();
            borrowed.return_value = None;
            borrowed.closure = None;
            #[cfg(not(stackful_coroutines))]
            borrowed.buffered.clear();
            true
        });
        stats.collected += sweep_vec(&mut heap.proxies, &marker.marked, |_| true);
        stats.collected += sweep_vec(&mut heap.bindings, &marker.marked, |cell| {
            let Ok(mut borrowed) = cell.try_borrow_mut() else {
                return false;
            };
            *borrowed = Value::Undefined;
            true
        });
        #[cfg(stackful_coroutines)]
        {
            stats.collected += sweep_vec(&mut heap.async_tasks, &marker.marked, |_| true);
        }
        heap.total_collected += stats.collected as u64;
    });
    stats
}

/// Registry size and lifetime collection total, for observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeapCounters {
    pub tracked: usize,
    pub total_collected: u64,
}

pub fn counters() -> HeapCounters {
    HEAP.with(|heap| {
        let heap = heap.borrow();
        HeapCounters {
            tracked: heap.tracked_count(),
            total_collected: heap.total_collected,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Interpreter;

    /// A fresh interpreter with the registry drained, so each test counts
    /// only the garbage its own script leaves behind. The registry is
    /// thread-local and shared with whatever tests ran on this thread
    /// before, so a leading collection is what makes the counts exact.
    fn clean_interp() -> Interpreter {
        let mut interp = Interpreter::with_builtins();
        let _ = interp.collect_cycles();
        interp
    }

    #[test]
    fn object_cycle_reclaimed() {
        let mut interp = clean_interp();
        interp
            .eval_source("function f() { let a = {}; let b = {}; a.peer = b; b.peer = a; } f();")
            .unwrap();
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
        // Whichever entry sweeps first clears and its cascade frees the
        // partner before its entry is reached, so the pair counts once.
        assert!(
            stats.collected >= 1,
            "expected the a/b cycle, got {stats:?}"
        );
    }

    #[test]
    fn reachable_objects_survive() {
        let mut interp = clean_interp();
        interp
            .eval_source("globalThis.keep = { x: 41 }; globalThis.arr = [1, 2, 3];")
            .unwrap();
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
        assert_eq!(stats.collected, 0);
        let check = interp.eval_source("keep.x + arr.length;").unwrap();
        assert!(
            matches!(check, Value::Number(x) if x == 44.0),
            "got {check:?}"
        );
    }

    #[test]
    fn closure_cycle_reclaimed() {
        let mut interp = clean_interp();
        interp
            .eval_source("function f() { let o = {}; o.fn = function () { return o; }; } f();")
            .unwrap();
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
        // The object clears first and its cascade frees the function and
        // the captured env before their entries are swept.
        assert!(
            stats.collected >= 1,
            "expected the closure cycle, got {stats:?}"
        );
    }

    #[test]
    fn env_cleared_while_held_by_promise() {
        let mut interp = clean_interp();
        interp
            .eval_source(
                "function f() { let p = new Promise(() => {}); let q = p.then(() => p); } f();",
            )
            .unwrap();
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
        // Envs sweep before promises, so the captured env is still alive
        // (held through the reaction) when its entry is swept: it clears,
        // and the cascade frees both promises before their entries run.
        assert!(
            stats.collected >= 1,
            "expected the env cycle, got {stats:?}"
        );
    }

    #[test]
    fn pure_promise_cycle_reclaimed() {
        use crate::value::Reaction;
        let mut interp = clean_interp();
        let p1 = Value::pending_promise();
        let p2 = Value::pending_promise();
        p1.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Undefined,
            on_rejected: Value::Undefined,
            derived: p2.clone(),
            adopted: false,
        });
        p2.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Undefined,
            on_rejected: Value::Undefined,
            derived: p1.clone(),
            adopted: false,
        });
        drop(p1);
        drop(p2);
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
        // The first entry to sweep clears and its cascade frees the
        // partner, so the pair counts once.
        assert!(
            stats.collected >= 1,
            "expected the promise cycle, got {stats:?}"
        );
    }

    fn probe_collect(
        interp: &mut crate::interpreter::Interpreter,
        _: Value,
        _: Vec<Value>,
    ) -> Result<Value, crate::error::VmErr> {
        let stats = interp.collect_cycles();
        Ok(Value::String(
            match stats.skipped {
                Some(SkipReason::Executing) => "executing",
                Some(SkipReason::SuspendedTasks) => "suspended",
                None => "ran",
            }
            .to_string(),
        ))
    }

    #[test]
    fn collect_refused_while_executing() {
        let mut interp = clean_interp();
        interp.global.borrow_mut().declare(
            "__collect",
            Value::NativeFunction {
                name: Rc::from("__collect"),
                callable: probe_collect,
            },
            crate::interpreter::BindKind::Var,
            true,
        );
        let verdict = interp.eval_source("__collect();").unwrap();
        assert!(
            matches!(&verdict, Value::String(s) if s == "executing"),
            "got {verdict:?}"
        );
        // Quiescent again: a direct call runs.
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
    }

    #[cfg(stackful_coroutines)]
    #[test]
    fn suspended_generator_refuses() {
        let mut interp = clean_interp();
        interp
            .eval_source("function* g() { yield 1; yield 2; } globalThis.gen = g(); gen.next();")
            .unwrap();
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, Some(SkipReason::SuspendedTasks));
    }

    #[test]
    fn host_pin_survives_collection() {
        use std::rc::Weak;
        let mut interp = clean_interp();
        let value = interp
            .eval_source("globalThis.tmp = { v: 7 }; tmp;")
            .unwrap();
        let Value::Object { props } = &value else {
            panic!("expected object, got {value:?}")
        };
        let weak: Weak<ObjectCell> = Rc::downgrade(props);
        let pin = add_root(value);
        interp.eval_source("globalThis.tmp = undefined;").unwrap();
        let stats = interp.collect_cycles();
        assert_eq!(stats.skipped, None);
        assert!(
            weak.upgrade().is_some(),
            "pinned object was collected: {stats:?}"
        );
        remove_root(pin);
        let stats = interp.collect_cycles();
        assert!(
            weak.upgrade().is_none(),
            "unpinned garbage survived: {stats:?}"
        );
    }
}
