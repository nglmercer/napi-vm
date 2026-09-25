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
}
