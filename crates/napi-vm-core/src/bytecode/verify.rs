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
    BadCache {
        address: usize,
        index: u32,
    },
    CacheTableMismatch,
    /// `slots.len() != local_count`.
    SlotTableMismatch {
        slots: usize,
        local_count: u16,
    },
    /// Parameters must occupy the leading slots.
    ParametersExceedLocals {
        parameters: u16,
        locals: u16,
    },
    /// Nonzero upvalues need Phase F's capture machinery.
    UpvaluesUnsupported {
        count: u16,
    },
    /// Register operand outside `0..register_count`.
    BadRegister {
        address: usize,
        register: u16,
    },
    /// Slot operand outside `0..local_count`.
    BadSlot {
        address: usize,
        slot: u16,
    },
    /// Constant-pool operand outside the pool.
    BadConstant {
        address: usize,
        index: u16,
    },
    /// Constant of the wrong variant for the instruction.
    ConstantTypeMismatch {
        address: usize,
        expected: &'static str,
    },
    /// Jump target outside `0..=code.len()`. Landing exactly on
    /// `code.len()` is falling off the end, which the VM defines
    /// (completion value at top level, `undefined` in functions).
    BadJumpTarget {
        address: usize,
        target: u32,
    },
    /// Call/array operand range outside the register file.
    BadOperandRange {
        address: usize,
        start: u16,
        count: u16,
    },
    /// `&&`/`||`/`??`/`,` must lower to jumps, never reach the VM.
    ShortCircuitInBinary {
        address: usize,
        op: BinOp,
    },
    /// `++`/`--`/`delete` must use their dedicated instructions.
    TargetedUnary {
        address: usize,
        op: UnOp,
    },
    /// Plain `=` must use a store, never a compound instruction.
    PlainAssignInCompound {
        address: usize,
    },
    /// `++`/`--` delta must be +1 or -1.
    BadIncDelta {
        address: usize,
        delta: i8,
    },
    /// Template quasi count must be the hole count plus one.
    TemplateArityMismatch {
        address: usize,
        quasis: usize,
        argc: u16,
    },
    /// A defect inside a nested compiled function.
    NestedFunction {
        index: u16,
        error: Box<VerifyError>,
    },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::BadCache { address, index } => write!(
                f,
                "bytecode verify failed at {address}: cache {index} out of range"
            ),
            VerifyError::CacheTableMismatch => {
                write!(f, "bytecode verify failed: property cache table mismatch")
            }
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
            VerifyError::BadOperandRange {
                address,
                start,
                count,
            } => write!(
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
            VerifyError::TemplateArityMismatch {
                address,
                quasis,
                argc,
            } => write!(
                f,
                "bytecode verify failed at {address}: {quasis} quasis for {argc} template holes"
            ),
            VerifyError::NestedFunction { index, error } => {
                write!(
                    f,
                    "bytecode verify failed in nested function c{index}: {error}"
                )
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verify one compiled function and, recursively, its nested functions.
pub fn verify_function(function: &BytecodeFunction) -> Result<(), VerifyError> {
    let sites = function
        .code
        .iter()
        .filter(|i| matches!(i, Instr::GetProp { .. } | Instr::SetProp { .. }))
        .count();
    if sites != function.caches.len() {
        return Err(VerifyError::CacheTableMismatch);
    }

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

    fn check_const_is(
        &self,
        address: usize,
        index: u16,
        expected: &'static str,
    ) -> Result<(), VerifyError> {
        self.check_const(address, index)?;
        let ok = match &self.function.constants[index as usize] {
            Constant::Number(_)
            | Constant::Bool(_)
            | Constant::Null
            | Constant::Undefined
            | Constant::BigInt(_)
            | Constant::Regex { .. } => expected == "scalar",
            // Strings load as values and name globals.
            Constant::String(_) | Constant::Name(_) => expected == "string" || expected == "scalar",
            Constant::StringList(_) => expected == "string-list",
            Constant::Function(_) => expected == "function",
            Constant::AstFunction(_) => expected == "ast-function",
            Constant::ObjectTemplate(_) => expected == "object-template",
            Constant::SpreadTemplate(_) => expected == "spread-template",
            Constant::ClassTemplate(_) => expected == "class-template",
            Constant::ImportTemplate(_) => expected == "import-template",
            Constant::ExportNamedTemplate(_) => expected == "export-named-template",
            Constant::ExportAllTemplate(_) => expected == "export-all-template",
        };
        if ok {
            Ok(())
        } else {
            Err(VerifyError::ConstantTypeMismatch { address, expected })
        }
    }

    fn check_target(&self, address: usize, target: u32) -> Result<(), VerifyError> {
        if (target as usize) <= self.function.code.len() {
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
            Err(VerifyError::BadOperandRange {
                address,
                start,
                count,
            })
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
                self.check_const_is(address, *cst, "scalar")?;
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
            Instr::DeclareLocal { slot, .. } | Instr::BareVarLocal { slot } => {
                self.check_slot(address, *slot)?;
            }
            Instr::InitLocal { slot, src } => {
                self.check_slot(address, *slot)?;
                self.check_reg(address, *src)?;
            }
            Instr::InitGlobal { name, src, .. } => {
                self.check_const_is(address, *name, "string")?;
                self.check_reg(address, *src)?;
            }
            Instr::HoistVarGlobal { name } | Instr::BareVarGlobal { name } => {
                self.check_const_is(address, *name, "string")?;
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
                // `++`/`--` on non-targets evaluates, then converts without
                // storing — the evaluator's fallback arm. Only `delete` has
                // no meaning here: it always needs its reference.
                if matches!(op, UnOp::Delete) {
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
            Instr::CompoundProp {
                dst,
                obj,
                key,
                op,
                rhs,
            } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
                self.check_reg(address, *rhs)?;
                self.check_compound_op(address, *op)?;
            }
            Instr::IncLocal {
                dst, slot, delta, ..
            } => {
                self.check_reg(address, *dst)?;
                self.check_slot(address, *slot)?;
                self.check_delta(address, *delta)?;
            }
            Instr::IncGlobal {
                dst, name, delta, ..
            } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *name, "string")?;
                self.check_delta(address, *delta)?;
            }
            Instr::IncProp {
                dst,
                obj,
                key,
                delta,
                ..
            } => {
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
            Instr::JumpIfTrue { src, target }
            | Instr::JumpIfFalse { src, target }
            | Instr::JumpIfNullish { src, target }
            | Instr::JumpIfNotNullish { src, target } => {
                self.check_reg(address, *src)?;
                self.check_target(address, *target)?;
            }
            Instr::LoopHead | Instr::ReturnUndefined => {}
            Instr::Return { src } | Instr::Throw { src } => {
                self.check_reg(address, *src)?;
            }
            Instr::GetProp {
                dst,
                obj,
                key,
                cache,
                ..
            } => {
                if *cache as usize >= self.function.caches.len() {
                    return Err(VerifyError::BadCache {
                        address,
                        index: *cache,
                    });
                }
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
            }
            Instr::SetProp {
                obj,
                key,
                val,
                cache,
            } => {
                if *cache as usize >= self.function.caches.len() {
                    return Err(VerifyError::BadCache {
                        address,
                        index: *cache,
                    });
                }
                self.check_reg(address, *obj)?;
                self.check_reg(address, *key)?;
                self.check_reg(address, *val)?;
            }
            Instr::DirectEval {
                dst,
                callee,
                args,
                argc,
            }
            | Instr::Call {
                dst,
                callee,
                args,
                argc,
            }
            | Instr::Construct {
                dst,
                callee,
                args,
                argc,
            } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *callee)?;
                self.check_range(address, *args, *argc)?;
            }
            Instr::CallMethod {
                dst,
                callee,
                this,
                args,
                argc,
            } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *callee)?;
                self.check_reg(address, *this)?;
                self.check_range(address, *args, *argc)?;
            }
            Instr::Template {
                dst,
                quasis,
                args,
                argc,
            } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *quasis, "string-list")?;
                self.check_range(address, *args, *argc)?;
                if let Constant::StringList(chunks) = &self.function.constants[*quasis as usize]
                    && chunks.len() != *argc as usize + 1
                {
                    return Err(VerifyError::TemplateArityMismatch {
                        address,
                        quasis: chunks.len(),
                        argc: *argc,
                    });
                }
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
            Instr::ExpandSpread { dst, src } | Instr::NormalKey { dst, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::BuildObject { dst, tmpl } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *tmpl, "object-template")?;
                if let Constant::ObjectTemplate(entries) = &self.function.constants[*tmpl as usize]
                {
                    for entry in entries {
                        if let Some(key) = entry.key {
                            self.check_key(address, key)?;
                        }
                        self.check_reg(address, entry.val)?;
                    }
                }
            }
            Instr::LoadGlobalSoft { dst, name } => {
                self.check_reg(address, *dst)?;
                self.check_const_is(address, *name, "string")?;
            }
            Instr::LoadLocalSoft { dst, slot } => {
                self.check_reg(address, *dst)?;
                self.check_slot(address, *slot)?;
            }
            Instr::DirectEvalSpread { dst, callee, tmpl }
            | Instr::ConstructSpread { dst, callee, tmpl }
            | Instr::CallSpread { dst, callee, tmpl } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *callee)?;
                self.check_spread_template(address, *tmpl)?;
            }
            Instr::MethodSpread {
                dst,
                callee,
                this,
                tmpl,
            } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *callee)?;
                self.check_reg(address, *this)?;
                self.check_spread_template(address, *tmpl)?;
            }
            Instr::BuildArray { dst, tmpl } => {
                self.check_reg(address, *dst)?;
                self.check_spread_template(address, *tmpl)?;
            }
            Instr::ToDestructArray { dst, src } | Instr::CheckDestructObject { dst, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::RestArray { dst, src, .. } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::RestObject {
                dst,
                src,
                keys,
                taken,
            } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
                self.check_reg(address, *keys)?;
                self.check_reg(address, *taken)?;
            }
            Instr::EnumKeys { dst, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::ForOfInit { iter, next, src } => {
                self.check_reg(address, *iter)?;
                self.check_reg(address, *next)?;
                self.check_reg(address, *src)?;
            }
            Instr::IterNext {
                done,
                value,
                iter,
                next,
            } => {
                self.check_reg(address, *done)?;
                self.check_reg(address, *value)?;
                self.check_reg(address, *iter)?;
                self.check_reg(address, *next)?;
            }
            Instr::CloseIterator { src, .. } => {
                self.check_reg(address, *src)?;
            }
            Instr::PushCatch { target, dst } | Instr::PushFinally { target, dst } => {
                self.check_target(address, *target)?;
                self.check_reg(address, *dst)?;
            }
            Instr::PopHandler | Instr::Rethrow => {}
            Instr::SuperMember { dst, key } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *key)?;
            }
            Instr::SuperReference {
                base,
                receiver,
                key,
                src,
            } => {
                for reg in [base, receiver, key, src] {
                    self.check_reg(address, *reg)?;
                }
            }
            Instr::GetPropertyWithReceiver {
                dst,
                base,
                receiver,
                key,
            } => {
                for reg in [dst, base, receiver, key] {
                    self.check_reg(address, *reg)?;
                }
            }
            Instr::SetPropertyWithReceiver {
                base,
                receiver,
                key,
                value,
            } => {
                for reg in [base, receiver, key, value] {
                    self.check_reg(address, *reg)?;
                }
            }
            Instr::NumericUpdate {
                previous,
                updated,
                src,
                ..
            } => {
                for reg in [previous, updated, src] {
                    self.check_reg(address, *reg)?;
                }
            }
            Instr::SuperConstructor { dst } => self.check_reg(address, *dst)?,
            Instr::SuperCall {
                dst,
                callee,
                args,
                argc,
            } => {
                self.check_reg(address, *callee)?;
                self.check_reg(address, *dst)?;
                self.check_range(address, *args, *argc)?;
            }
            Instr::SuperCallSpread { dst, callee, tmpl } => {
                self.check_reg(address, *callee)?;
                self.check_reg(address, *dst)?;
                self.check_spread_template(address, *tmpl)?;
            }
            Instr::Raise { msg } => {
                self.check_const_is(address, *msg, "string")?;
            }
            Instr::BuildClass { dst, tmpl } => {
                self.check_reg(address, *dst)?;
                self.check_class_template(address, *tmpl)?;
            }
            Instr::PropertyKey { dst, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::ClassScope { name } => {
                if let Some(name) = name {
                    self.check_const_is(address, *name, "string")?;
                }
            }
            Instr::ClassPrivateEnvironment { names } => {
                self.check_const_is(address, *names, "string-list")?
            }
            Instr::ClassHeritage { dst, superclass } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *superclass)?;
            }
            Instr::PrivateIn { dst, obj, name } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *obj)?;
                self.check_const_is(address, *name, "string")?;
            }
            Instr::Import { tmpl } => {
                self.check_const_is(address, *tmpl, "import-template")?;
            }
            Instr::ExportDefault { src } => {
                self.check_reg(address, *src)?;
            }
            Instr::ExportNamed { tmpl } => {
                self.check_const_is(address, *tmpl, "export-named-template")?;
            }
            Instr::ExportAll { tmpl } => {
                self.check_const_is(address, *tmpl, "export-all-template")?;
            }
            Instr::DynamicImport { dst, src } | Instr::Await { dst, src } => {
                self.check_reg(address, *dst)?;
                self.check_reg(address, *src)?;
            }
            Instr::NewTarget { dst } | Instr::ImportMeta { dst } => {
                self.check_reg(address, *dst)?;
            }
            // Scope balance is a compiler invariant, like handler balance:
            // an underflow fails loudly at runtime as an internal error.
            Instr::PushScope | Instr::PopScope => {}
        }
        Ok(())
    }
}

impl Checker<'_> {
    fn check_spread_template(&self, address: usize, tmpl: u16) -> Result<(), VerifyError> {
        self.check_const_is(address, tmpl, "spread-template")?;
        if let Constant::SpreadTemplate(entries) = &self.function.constants[tmpl as usize] {
            for entry in entries {
                self.check_reg(address, entry.reg)?;
            }
        }
        Ok(())
    }

    fn check_class_template(&self, address: usize, tmpl: u16) -> Result<(), VerifyError> {
        use super::constants::ClassNameTemplate;
        self.check_const_is(address, tmpl, "class-template")?;
        let Constant::ClassTemplate(template) = &self.function.constants[tmpl as usize] else {
            return Ok(());
        };
        if let Some(reg) = template.superclass {
            self.check_reg(address, reg)?;
        }
        if let Some(reg) = template.super_proto {
            self.check_reg(address, reg)?;
        }
        for reg in &template.ctor_computed_keys {
            self.check_reg(address, *reg)?;
        }
        self.check_func_const(address, template.ctor_func)?;
        for member in &template.members {
            if let ClassNameTemplate::Computed(reg) = &member.name {
                self.check_reg(address, *reg)?;
            }
            if let Some(func) = member.func {
                self.check_func_const(address, func)?;
            }
        }
        for block in &template.blocks {
            match block {
                super::constants::ClassStaticTemplate::Block(index) => {
                    self.check_const_is(address, *index, "ast-function")?
                }
                super::constants::ClassStaticTemplate::Field {
                    name: ClassNameTemplate::Computed(reg),
                    ..
                } => self.check_reg(address, *reg)?,
                super::constants::ClassStaticTemplate::Field { .. } => {}
            }
        }
        Ok(())
    }

    /// A function constant in either form: compiled bytecode or an AST
    /// fallback for members the compiler declined.
    fn check_func_const(&self, address: usize, index: u16) -> Result<(), VerifyError> {
        self.check_const(address, index)?;
        match &self.function.constants[index as usize] {
            Constant::Function(_) | Constant::AstFunction(_) => Ok(()),
            _ => Err(VerifyError::ConstantTypeMismatch {
                address,
                expected: "function",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::compiler::compile_program;
    use crate::parser::parse_cached;

    fn compile(source: &str) -> BytecodeFunction {
        let stmts = parse_cached(source).expect("test source must parse");
        let module = compile_program(&stmts).expect("test source must compile");
        (*module.main).clone()
    }

    #[test]
    fn load_const_rejects_non_scalar_constants() {
        let mut unit = compile("1");
        let index = unit
            .constants
            .iter()
            .position(|c| matches!(c, Constant::Number(_)))
            .expect("number const");
        unit.constants[index] = Constant::StringList(vec!["q".into()]);
        assert!(matches!(
            verify_function(&unit),
            Err(VerifyError::ConstantTypeMismatch {
                expected: "scalar",
                ..
            })
        ));
    }

    #[test]
    fn template_rejects_quasi_arity_mismatch() {
        let mut unit = compile("`a${x}b`");
        let index = unit
            .constants
            .iter()
            .position(|c| matches!(c, Constant::StringList(_)))
            .expect("quasis const");
        if let Constant::StringList(quasis) = &mut unit.constants[index] {
            quasis.pop();
        }
        assert!(matches!(
            verify_function(&unit),
            Err(VerifyError::TemplateArityMismatch {
                quasis: 1,
                argc: 1,
                ..
            })
        ));
    }

    #[test]
    fn private_brand_instruction_rejects_invalid_operands() {
        let original = compile("1");
        for (dst, obj, name) in [
            (original.register_count, 0, 0),
            (0, original.register_count, 0),
            (0, 0, u16::MAX),
            (0, 0, 0),
        ] {
            let mut unit = original.clone();
            unit.code[0] = Instr::PrivateIn { dst, obj, name };
            assert!(verify_function(&unit).is_err());
        }
    }
    #[test]
    fn receiver_operations_reject_every_out_of_range_register() {
        let original = compile("1");
        let bad = original.register_count;
        let instructions = [
            Instr::SuperReference {
                base: bad,
                receiver: 0,
                key: 0,
                src: 0,
            },
            Instr::SuperReference {
                base: 0,
                receiver: bad,
                key: 0,
                src: 0,
            },
            Instr::SuperReference {
                base: 0,
                receiver: 0,
                key: bad,
                src: 0,
            },
            Instr::SuperReference {
                base: 0,
                receiver: 0,
                key: 0,
                src: bad,
            },
            Instr::GetPropertyWithReceiver {
                dst: bad,
                base: 0,
                receiver: 0,
                key: 0,
            },
            Instr::GetPropertyWithReceiver {
                dst: 0,
                base: bad,
                receiver: 0,
                key: 0,
            },
            Instr::GetPropertyWithReceiver {
                dst: 0,
                base: 0,
                receiver: bad,
                key: 0,
            },
            Instr::GetPropertyWithReceiver {
                dst: 0,
                base: 0,
                receiver: 0,
                key: bad,
            },
            Instr::SetPropertyWithReceiver {
                base: bad,
                receiver: 0,
                key: 0,
                value: 0,
            },
            Instr::SetPropertyWithReceiver {
                base: 0,
                receiver: bad,
                key: 0,
                value: 0,
            },
            Instr::SetPropertyWithReceiver {
                base: 0,
                receiver: 0,
                key: bad,
                value: 0,
            },
            Instr::SetPropertyWithReceiver {
                base: 0,
                receiver: 0,
                key: 0,
                value: bad,
            },
            Instr::NumericUpdate {
                previous: bad,
                updated: 0,
                src: 0,
                increment: true,
            },
            Instr::NumericUpdate {
                previous: 0,
                updated: bad,
                src: 0,
                increment: true,
            },
            Instr::NumericUpdate {
                previous: 0,
                updated: 0,
                src: bad,
                increment: true,
            },
            Instr::SuperConstructor { dst: bad },
            Instr::SuperCall {
                dst: 0,
                callee: bad,
                args: 0,
                argc: 0,
            },
        ];
        for instruction in instructions {
            let mut unit = original.clone();
            unit.code[0] = instruction;
            assert!(verify_function(&unit).is_err());
        }
    }
    #[test]
    fn spread_operations_reject_invalid_registers_and_templates() {
        let statements = crate::parser::parse_cached("function C(){}new C(...[1]);").unwrap();
        let module = crate::bytecode::compile_program(&statements).unwrap();
        let original = module.main.as_ref();
        let bad = original.register_count;
        for instruction in [
            Instr::ExpandSpread { dst: bad, src: 0 },
            Instr::ExpandSpread { dst: 0, src: bad },
            Instr::ConstructSpread {
                dst: bad,
                callee: 0,
                tmpl: 0,
            },
            Instr::ConstructSpread {
                dst: 0,
                callee: bad,
                tmpl: 0,
            },
            Instr::ConstructSpread {
                dst: 0,
                callee: 0,
                tmpl: u16::MAX,
            },
        ] {
            let mut unit = original.clone();
            unit.code[0] = instruction;
            assert!(verify_function(&unit).is_err());
        }
    }
}
