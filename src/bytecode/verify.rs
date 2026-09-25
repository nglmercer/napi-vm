//! Bytecode verifier (spec §21).
//!
//! Every compiled function passes through [`verify_function`] before it can
//! execute. The verifier proves the structural properties the VM's fast
//! paths rely on: all indices land inside their tables, all jumps land on
//! instruction boundaries, and no instruction carries an operand the VM
//! cannot execute. Verification failure is an internal error (the compiler
//! is currently the only producer); malformed bytecode can fail this check
//! but never causes unchecked indexing or Rust UB in the VM.

use super::constants::Constant;
use super::function::BytecodeFunction;
use super::module::BytecodeModule;
use super::opcode::{Instr, KeySrc};
use crate::parser::{AssignOp, BinOp, UnOp};

/// One structural defect found in a compiled function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// `slots.len() != local_count`.
    SlotTableMismatch { slots: usize, local_count: u16 },
    /// Parameters must occupy the leading slots.
    ParametersExceedLocals { parameters: u16, locals: u16 },
    /// Nonzero upvalues need Phase F's capture machinery.
    UpvaluesUnsupported { count: u16 },
    /// Register operand outside `0..register_count`.
    BadRegister { address: usize, register: u16 },
    /// Slot operand outside `0..local_count`.
    BadSlot { address: usize, slot: u16 },
    /// Constant-pool operand outside the pool.
    BadConstant { address: usize, index: u16 },
    /// Constant of the wrong variant for the instruction.
    ConstantTypeMismatch { address: usize, expected: &'static str },
    /// Jump target outside `0..code.len()`.
    BadJumpTarget { address: usize, target: u32 },
    /// Call/array operand range outside the register file.
    BadOperandRange { address: usize, start: u16, count: u16 },
    /// `&&`/`||`/`??`/`,` must lower to jumps, never reach the VM.
    ShortCircuitInBinary { address: usize, op: BinOp },
    /// `++`/`--`/`delete` must use their dedicated instructions.
    TargetedUnary { address: usize, op: UnOp },
    /// Plain `=` must use a store, never a compound instruction.
    PlainAssignInCompound { address: usize },
    /// `++`/`--` delta must be +1 or -1.
    BadIncDelta { address: usize, delta: i8 },
    /// A defect inside a nested compiled function.
    NestedFunction { index: u16, error: Box<VerifyError> },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::SlotTableMismatch { slots, local_count } => write!(
                f,
                "bytecode verify failed: {slots} slot infos for {local_count} locals"
            ),
            VerifyError::ParametersExceedLocals { parameters, locals } => write!(
                f,
                "bytecode verify failed: {parameters} parameters exceed {locals} locals"
            ),
            VerifyError::UpvaluesUnsupported { count } => write!(
                f,
                "bytecode verify failed: {count} upvalues need Phase F support"
            ),
            VerifyError::BadRegister { address, register } => write!(
                f,
                "bytecode verify failed at {address}: register r{register} out of range"
            ),
            VerifyError::BadSlot { address, slot } => write!(
                f,
                "bytecode verify failed at {address}: slot s{slot} out of range"
            ),
            VerifyError::BadConstant { address, index } => write!(
                f,
                "bytecode verify failed at {address}: constant c{index} out of range"
            ),
            VerifyError::ConstantTypeMismatch { address, expected } => write!(
                f,
                "bytecode verify failed at {address}: constant is not {expected}"
            ),
            VerifyError::BadJumpTarget { address, target } => write!(
                f,
                "bytecode verify failed at {address}: jump target @{target} out of range"
            ),
            VerifyError::BadOperandRange { address, start, count } => write!(
                f,
                "bytecode verify failed at {address}: registers r{start}..+{count} out of range"
            ),
            VerifyError::ShortCircuitInBinary { address, op } => write!(
                f,
                "bytecode verify failed at {address}: {op:?} must lower to jumps"
            ),
            VerifyError::TargetedUnary { address, op } => write!(
                f,
                "bytecode verify failed at {address}: {op:?} needs a targeted instruction"
            ),
            VerifyError::PlainAssignInCompound { address } => write!(
                f,
                "bytecode verify failed at {address}: plain `=` needs a store instruction"
            ),
            VerifyError::BadIncDelta { address, delta } => write!(
                f,
                "bytecode verify failed at {address}: inc delta {delta} is not +1/-1"
            ),
            VerifyError::NestedFunction { index, error } => {
                write!(f, "bytecode verify failed in nested function c{index}: {error}")
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verify one compiled function and, recursively, its nested functions.
pub fn verify_function(function: &BytecodeFunction) -> Result<(), VerifyError> {
    if function.slots.len() != function.local_count as usize {
        return Err(VerifyError::SlotTableMismatch {
            slots: function.slots.len(),
            local_count: function.local_count,
        });
    }
    if function.parameter_count > function.local_count {
        return Err(VerifyError::ParametersExceedLocals {
            parameters: function.parameter_count,
            locals: function.local_count,
        });
    }
    if function.upvalue_count != 0 {
        return Err(VerifyError::UpvaluesUnsupported {
            count: function.upvalue_count,
        });
    }
    let checker = Checker { function };
    for (address, instr) in function.code.iter().enumerate() {
        checker.check_instr(address, instr)?;
    }
    for (index, constant) in function.constants.iter().enumerate() {
        if let Constant::Function(nested) = constant {
            verify_function(nested).map_err(|error| VerifyError::NestedFunction {
                index: index as u16,
                error: Box::new(error),
            })?;
        }
    }
    Ok(())
}

/// Verify a whole compiled module (currently just its main function; the
/// recursion into nested functions happens in [`verify_function`]).
pub fn verify_module(module: &BytecodeModule) -> Result<(), VerifyError> {
    verify_function(&module.main)
}

struct Checker<'a> {
    function: &'a BytecodeFunction,
}

impl Checker<'_> {
    fn check_reg(&self, address: usize, reg: u16) -> Result<(), VerifyError> {
        if reg < self.function.register_count {
            Ok(())
        } else {
            Err(VerifyError::BadRegister {
                address,
                register: reg,
            })
        }
    }

    fn check_slot(&self, address: usize, slot: u16) -> Result<(), VerifyError> {
        if slot < self.function.local_count {
            Ok(())
        } else {
            Err(VerifyError::BadSlot { address, slot })
        }
    }

    fn check_const(&self, address: usize, index: u16) -> Result<(), VerifyError> {
        if (index as usize) < self.function.constants.len() {
            Ok(())
        } else {
            Err(VerifyError::BadConstant { address, index })
        }
    }

    fn check_const_is(&self, address: usize, index: u16, expected: &'static str) -> Result<(), VerifyError> {
        self.check_const(address, index)?;
        let ok = match &self.function.constants[index as usize] {
            Constant::String(_) => expected == "string",
            Constant::Function(_) => expected == "function",
            Constant::AstFunction(_) => expected == "ast-function",
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(VerifyError::ConstantTypeMismatch { address, expected })
        }
    }

    fn check_target(&self, address: usize, target: u32) -> Result<(), VerifyError> {
        if (target as usize) < self.function.code.len() {
            Ok(())
        } else {
            Err(VerifyError::BadJumpTarget { address, target })
        }
    }

    fn check_range(&self, address: usize, start: u16, count: u16) -> Result<(), VerifyError> {
        let end = start as u32 + count as u32;
        if end <= self.function.register_count as u32 {
            Ok(())
        } else {
            Err(VerifyError::BadOperandRange { address, start, count })
        }
    }

    fn check_key(&self, address: usize, key: KeySrc) -> Result<(), VerifyError> {
        match key {
            KeySrc::Const(index) => self.check_const_is(address, index, "string"),
            KeySrc::Reg(reg) => self.check_reg(address, reg),
        }
    }

    fn check_compound_op(&self, address: usize, op: AssignOp) -> Result<(), VerifyError> {
        if op == AssignOp::Assign {
            Err(VerifyError::PlainAssignInCompound { address })
        } else {
            Ok(())
        }
    }

    fn check_delta(&self, address: usize, delta: i8) -> Result<(), VerifyError> {
        if delta == 1 || delta == -1 {
            Ok(())
        } else {
            Err(VerifyError::BadIncDelta { address, delta })
        }
    }

    fn check_instr(&self, address: usize, instr: &Instr) -> Result<(), VerifyError> {
        match instr {
            Instr::LoadConst { dst, cst } => {
                self.check_reg(address, *dst)?;
                self.check_const(address, *cst)?;
            }
            Instr::Mov { dst, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::LoadLocal { dst, slot } | Instr::TypeofLocal { dst, slot } => {
                self.check_reg(address, *dst)?;
                self.check_slot(address, *slot)?;
            }
            Instr::StoreLocal { slot, src } => {
                self.check_slot(address, *slot)?;
                self.check_reg(address, *src)?;
            }
            Instr::LoadGlobal { dst, name } | Instr::TypeofGlobal { dst, name } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *name, "string")?;
            }
            Instr::StoreGlobal { name, src } => {
                self.check_const_is(address, *name, "string")?;
                self.check_reg(address, *src)?;
            }
            Instr::DefineGlobal { name, src, .. } => {
                self.check_const_is(address, *name, "string")?;
                self.check_reg(address, *src)?;
            }
            Instr::LoadThis { dst } | Instr::LoadGlobalThis { dst } => {
                self.check_reg(address, *dst)?;
            }
            Instr::Binary { dst, op, lhs, rhs } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *lhs)?;
                self.check_reg(address, *rhs)?;
                if matches!(op, BinOp::And | BinOp::Or | BinOp::Nullish | BinOp::Comma) {
                    return Err(VerifyError::ShortCircuitInBinary { address, op: *op });
                }
            }
            Instr::Unary { dst, op, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
                if matches!(op, UnOp::Inc | UnOp::Dec | UnOp::Delete) {
                    return Err(VerifyError::TargetedUnary { address, op: *op });
                }
            }
            Instr::CompoundLocal { dst, slot, op, rhs } => {
                self.check_reg(address, *dst)?;
                self.check_slot(address, *slot)?;
                self.check_reg(address, *rhs)?;
                self.check_compound_op(address, *op)?;
            }
            Instr::CompoundGlobal { dst, name, op, rhs } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *name, "string")?;
                self.check_reg(address, *rhs)?;
                self.check_compound_op(address, *op)?;
            }
            Instr::CompoundProp { dst, obj, key, op, rhs } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
                self.check_reg(address, *rhs)?;
                self.check_compound_op(address, *op)?;
            }
            Instr::IncLocal { dst, slot, delta, .. } => {
                self.check_reg(address, *dst)?;
                self.check_slot(address, *slot)?;
                self.check_delta(address, *delta)?;
            }
            Instr::IncGlobal { dst, name, delta, .. } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *name, "string")?;
                self.check_delta(address, *delta)?;
            }
            Instr::IncProp { dst, obj, key, delta, .. } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
                self.check_delta(address, *delta)?;
            }
            Instr::DelProp { dst, obj, key } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
            }
            Instr::DelGlobal { dst, name } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *name, "string")?;
            }
            Instr::Jump { target } => {
                self.check_target(address, *target)?;
            }
            Instr::JumpIfTrue { src, target } | Instr::JumpIfFalse { src, target } => {
                self.check_reg(address, *src)?;
                self.check_target(address, *target)?;
            }
            Instr::LoopHead | Instr::ReturnUndefined => {}
            Instr::Return { src } | Instr::Throw { src } => {
                self.check_reg(address, *src)?;
            }
            Instr::GetProp { dst, obj, key } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
            }
            Instr::SetProp { obj, key, val } => {
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
                self.check_reg(address, *val)?;
            }
            Instr::Call { dst, callee, args, argc }
            | Instr::Construct { dst, callee, args, argc } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *callee)?;
                self.check_range(address, *args, *argc)?;
            }
            Instr::NewObject { dst } => {
                self.check_reg(address, *dst)?;
            }
            Instr::SetOwnProp { obj, key, val } => {
                self.check_reg(address, *obj)?;
                self.check_key(address, *key)?;
                self.check_reg(address, *val)?;
            }
            Instr::NewArray { dst, args, argc } => {
                self.check_reg(address, *dst)?;
                self.check_range(address, *args, *argc)?;
            }
            Instr::MakeFunction { dst, func } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *func, "function")?;
            }
            Instr::MakeAstFunction { dst, ast } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *ast, "ast-function")?;
            }
        }
        Ok(())
    }
}
