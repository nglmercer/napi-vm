//! Static representation of one compiled function.
//!
//! A [`BytecodeFunction`] is pure data: instruction stream, constant pool,
//! and the slot/register layout the VM needs to build a call frame. It
//! borrows nothing from any interpreter, so compiled units are shareable
//! across runtimes and threads.

use super::opcode::Instr;
use crate::bytecode::constants::Constant;

/// Declaration kind of one local slot, mirroring `BindKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Var,
    Let,
    Const,
}

/// Compile-time metadata for one local slot: the source name (for
/// `ReferenceError`/`TypeError` messages) and its declaration kind (for
/// temporal-dead-zone and const-assignment checks).
#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub name: String,
    pub kind: SlotKind,
    /// Whether a nested function may observe this slot. Captured slots live
    /// in the frame environment (where the closure chain reaches them) and
    /// compile to the global instruction family; the slot itself stays an
    /// untouched placeholder. Only function-root slots are ever captured.
    /// The verifier does not check this: a violation would surface as a
    /// loud dead-zone error, never unsoundness.
    pub captured: bool,
}

/// One compiled function (or top-level program, which compiles as a
/// zero-parameter function whose outer scope is the global environment).
#[derive(Debug, Clone)]
pub struct BytecodeFunction {
    /// Function name for stack traces; `None` for anonymous/top-level.
    pub name: Option<String>,
    /// Instruction stream. Jump targets are indices into this vector.
    pub code: Vec<Instr>,
    /// Constant pool. See [`Constant`].
    pub constants: Vec<Constant>,
    /// Expression temporaries per frame. Registers are always initialized:
    /// the compiler never emits a read before a write.
    pub register_count: u16,
    /// Local slots per frame; always `slots.len()`.
    pub local_count: u16,
    /// Leading slots (`0..parameter_count`) hold parameters.
    pub parameter_count: u16,
    /// Captured-variable slots. Always zero in Phase E: capturing
    /// functions decline compilation (upvalues arrive in Phase F).
    pub upvalue_count: u16,
    /// Per-slot metadata, indexed by slot.
    pub slots: Vec<SlotInfo>,
    /// Arrow functions bind `this` lexically (top-level arrows only in E).
    pub is_arrow: bool,
    /// Whether `new` accepts this function (plain declarations/expressions).
    pub is_constructor: bool,
    /// This function — or a nested arrow — reads `arguments` through the
    /// scope chain. Non-arrow calls seed the arguments object into the
    /// frame environment; arrows never seed (they would shadow the
    /// captured one) and pass the need outward to their definer instead.
    pub captures_arguments: bool,
    /// Per-instruction property inline caches, parallel to `code`: only
    /// `GetProp`/`SetProp` sites use their slot, the rest stay empty.
    /// Cells only, so the VM probes and fills with plain loads and stores
    /// and no borrow can span a re-entrant slow path.
    pub caches: Box<[crate::shape::PropCache]>,
    /// Tier-up state for the JIT seam: entry/loop hotness plus the cached
    /// compilation, if a backend produced one. Cloning a function forks
    /// its tier state; each copy counts and compiles independently.
    pub tiers: crate::jit::TierCounters,
}

/// Tier-up and inline-cache counters for one function tree: this function
/// plus every nested bytecode function in its constants, recursively.
/// AST-backed nested functions contribute nothing — they have no tier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionStats {
    /// Entries through the tier-up check (top-level programs included).
    pub calls: u32,
    /// Loop-head ticks: iterations plus one entry tick per loop, mirroring
    /// the loop budget.
    pub loop_iters: u64,
    /// Functions currently holding a compiled artifact.
    pub compiled: u32,
    /// Guard failures since measurement began.
    pub deopts: u32,
    /// Property sites (`GetProp`/`SetProp` instructions).
    pub ic_sites: usize,
    /// Inline-cache hits and misses across those sites.
    pub ic_hits: u32,
    pub ic_misses: u32,
    /// Sites that stopped caching after seeing too many shapes.
    pub ic_mega_sites: usize,
}

impl FunctionStats {
    /// Add another tree's counters into this one.
    pub fn merge(&mut self, other: &Self) {
        self.calls = self.calls.saturating_add(other.calls);
        self.loop_iters = self.loop_iters.saturating_add(other.loop_iters);
        self.compiled = self.compiled.saturating_add(other.compiled);
        self.deopts = self.deopts.saturating_add(other.deopts);
        self.ic_sites += other.ic_sites;
        self.ic_hits = self.ic_hits.saturating_add(other.ic_hits);
        self.ic_misses = self.ic_misses.saturating_add(other.ic_misses);
        self.ic_mega_sites += other.ic_mega_sites;
    }
}

impl BytecodeFunction {
    /// Render the instruction stream with addresses, for tests and debugging.
    pub fn disassemble(&self) -> String {
        let mut out = String::new();
        for (address, instr) in self.code.iter().enumerate() {
            out.push_str(&format!("{address:04} {instr}\n"));
        }
        out
    }

    /// Snapshot this function tree's tier-up and inline-cache counters.
    pub fn stats(&self) -> FunctionStats {
        let mut stats = FunctionStats {
            calls: self.tiers.calls.get(),
            loop_iters: self.tiers.loop_iters.get(),
            ..FunctionStats::default()
        };
        if let Some(code) = self.tiers.code.borrow().as_ref().and_then(|c| c.as_ref()) {
            stats.compiled = 1;
            stats.deopts = code.deopts.get();
        }
        for (index, instr) in self.code.iter().enumerate() {
            if !matches!(instr, Instr::GetProp { .. } | Instr::SetProp { .. }) {
                continue;
            }
            stats.ic_sites += 1;
            let Some(cache) = self.caches.get(index) else {
                continue;
            };
            let (hits, misses) = cache.stats();
            stats.ic_hits = stats.ic_hits.saturating_add(hits);
            stats.ic_misses = stats.ic_misses.saturating_add(misses);
            stats.ic_mega_sites += usize::from(cache.is_megamorphic());
        }
        for constant in &self.constants {
            if let Constant::Function(nested) = constant {
                stats.merge(&nested.stats());
            }
        }
        stats
    }
}
