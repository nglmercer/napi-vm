//! Bytecode virtual machine: executes verified [`BytecodeFunction`]s.
//!
//! The VM is a thin dispatch loop over the [`Instr`] set. It owns no
//! semantics of its own: operators, property access, calls, and scope
//! operations all delegate to the same interpreter helpers the AST
//! evaluator calls, so the two tiers agree by construction. Local slots
//! mirror [`Environment`] binding rules (temporal dead zone, const
//! assignment) instruction by instruction; top-level outer bindings operate
//! on the real global environment.
//!
//! The VM trusts verification: [`compile_program`](super::compiler::compile_program)
//! output is verified before it can execute, so register, slot, constant,
//! and jump-target indices are used directly. Anything verification cannot
//! prove (the instruction pointer itself, constant types at polymorphic
//! sites) fails loudly with an internal error — never a panic, never Rust
//! UB, and never a silent wrong result.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{
    BindKind, Env, Environment, Interpreter, Lookup, ObjectAccessorKind,
    insert_object_property, intern_params, push_call_arg, symbol_slot_key,
};
use crate::value::{FunctionData, PropAttrs, Value};

use super::constants::{Constant, PropEntry, PropKind, SpreadEntry};
use super::function::{BytecodeFunction, SlotKind};
use super::module::BytecodeModule;
use super::opcode::{Instr, KeySrc, Reg, Slot};

/// One local slot at runtime: the value plus its declaration facts.
#[derive(Debug, Clone)]
pub struct RunSlot {
    pub value: Value,
    pub initialized: bool,
    pub kind: SlotKind,
}

/// One activation of a bytecode function: registers, slots, and `this`.
pub struct CallFrame<'a> {
    pub function: &'a BytecodeFunction,
    pub ip: usize,
    pub registers: Vec<Value>,
    pub slots: Vec<RunSlot>,
    pub this_value: Value,
    handlers: Vec<HandlerEntry>,
    pending: Option<VmErr>,
}

/// One pushed exception handler: `target` resumes after an interception,
/// `dst` receives the catch value (catch handlers), `catch_returns`
/// selects finally handlers, which also intercept `return` unwinding.
struct HandlerEntry {
    target: u32,
    dst: Reg,
    catch_returns: bool,
}

impl<'a> CallFrame<'a> {
    fn setup(function: &'a BytecodeFunction, this_value: Value, args: &[Value]) -> Self {
        let registers = vec![Value::Undefined; function.register_count as usize];
        let mut slots = Vec::with_capacity(function.local_count as usize);
        for (index, info) in function.slots.iter().enumerate() {
            // Captured slots live in the frame environment (boxed at call
            // time); the slot stays an untouched placeholder.
            if info.captured {
                slots.push(RunSlot {
                    value: Value::Undefined,
                    initialized: false,
                    kind: info.kind,
                });
                continue;
            }
            // Parameters bind their arguments; a parameter slot merged with
            // a lexical declaration discards the argument and starts dead,
            // mirroring hoisting. Plain `var` slots start defined.
            if index < function.parameter_count as usize && info.kind == SlotKind::Var {
                slots.push(RunSlot {
                    value: args.get(index).cloned().unwrap_or(Value::Undefined),
                    initialized: true,
                    kind: info.kind,
                });
            } else if info.kind == SlotKind::Var {
                slots.push(RunSlot {
                    value: Value::Undefined,
                    initialized: true,
                    kind: info.kind,
                });
            } else {
                slots.push(RunSlot {
                    value: Value::Undefined,
                    initialized: false,
                    kind: info.kind,
                });
            }
        }
        Self { function, ip: 0, registers, slots, this_value, handlers: Vec::new(), pending: None }
    }
}

fn internal(what: &str) -> VmErr {
    VmErr::Msg(format!("internal error: {what}"))
}

fn const_string(function: &BytecodeFunction, index: u16) -> Result<&str, VmErr> {
    match function.constants.get(index as usize) {
        Some(Constant::String(name)) => Ok(name),
        _ => Err(internal("bad string constant")),
    }
}

/// Execute a top-level module. Falling off the end yields register zero,
/// the program completion value; `return` escapes as `VmErr::Ret`, exactly
/// like the AST evaluator's `run`.
pub(crate) fn run_module(interp: &mut Interpreter, module: &BytecodeModule) -> Result<Value, VmErr> {
    let mut frame = CallFrame::setup(&module.main, Value::Undefined, &[]);
    run_loop(interp, &mut frame, true)
}

/// Call a bytecode function with freshly bound parameter slots. Falling off
/// the end yields `undefined`; `return` signals `VmErr::Ret`, like the
/// evaluator's bodies. The frame environment carries the captured slots so
/// nested closures observe them through the scope chain; locals stay in
/// slots. The previous scope is restored on every path, including errors.
pub(crate) fn run_function(
    interp: &mut Interpreter,
    code: &BytecodeFunction,
    parent_env: Env,
    this_value: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let fe = Rc::new(RefCell::new(Environment::child(parent_env)));
    if !code.is_arrow {
        fe.borrow_mut().set("this", this_value.clone());
    }
    seed_captured(&fe, code, &args);
    let saved = std::mem::replace(&mut interp.global, fe);
    let mut frame = CallFrame::setup(code, this_value, &args);
    let result = run_loop(interp, &mut frame, false);
    interp.global = saved;
    result
}

/// Box the captured slots into a fresh frame environment, mirroring
/// [`CallFrame::setup`]'s merge rules: parameters bind their arguments
/// unless a lexical declaration merged the slot dead, plain `var`s start
/// defined, lexicals dead. Hoisted-function cells are overwritten by the
/// eager instantiation when the body starts.
fn seed_captured(fe: &Env, code: &BytecodeFunction, args: &[Value]) {
    for (index, info) in code.slots.iter().enumerate() {
        if !info.captured {
            continue;
        }
        let (value, initialized) = if index < code.parameter_count as usize
            && info.kind == SlotKind::Var
        {
            (args.get(index).cloned().unwrap_or(Value::Undefined), true)
        } else if info.kind == SlotKind::Var {
            (Value::Undefined, true)
        } else {
            (Value::Undefined, false)
        };
        fe.borrow_mut().declare(&info.name, value, bind_kind(info.kind), initialized);
    }
}

fn run_loop(
    interp: &mut Interpreter,
    frame: &mut CallFrame,
    top_level: bool,
) -> Result<Value, VmErr> {
    loop {
        let instr = match frame.function.code.get(frame.ip) {
            Some(instr) => instr.clone(),
            // Falling off the end is legitimate (a body without `return`);
            // jumping past it is a compiler bug and fails loudly.
            None if frame.ip == frame.function.code.len() => {
                if top_level {
                    return Ok(frame.registers.first().cloned().unwrap_or(Value::Undefined));
                }
                return Ok(Value::Undefined);
            }
            None => return Err(internal("instruction pointer out of bounds")),
        };
        // Every instruction runs inside a closure so handler
        // interception sees each error exactly once; the `return`s in the
        // arms below exit the closure, not the loop.
        let outcome = (|| -> Result<(), VmErr> {
            interp.consume_fuel(instr.cost())?;
            frame.ip += 1;
            match instr {
            Instr::LoadConst { dst, cst } => {
                let value = match &frame.function.constants[cst as usize] {
                    Constant::Number(n) => Value::Number(*n),
                    Constant::String(s) => Value::String(s.clone()),
                    Constant::Bool(b) => Value::Bool(*b),
                    Constant::Null => Value::Null,
                    Constant::Undefined => Value::Undefined,
                    Constant::BigInt(v) => Value::BigInt(v.clone()),
                    Constant::Regex { pattern, flags } => {
                        crate::builtins::compile_regex(pattern, flags)?
                    }
                    _ => return Err(internal("invalid load_const")),
                };
                frame.registers[dst as usize] = value;
            }
            Instr::Mov { dst, src } => {
                frame.registers[dst as usize] = frame.registers[src as usize].clone();
            }
            Instr::LoadLocal { dst, slot } => {
                let slot_value = &frame.slots[slot as usize];
                if !slot_value.initialized {
                    return Err(VmErr::Msg(format!(
                        "ReferenceError: Cannot access '{}' before initialization",
                        frame.function.slots[slot as usize].name
                    )));
                }
                frame.registers[dst as usize] = slot_value.value.clone();
            }
            Instr::StoreLocal { slot, src } => {
                check_slot_writable(frame, slot)?;
                let value = frame.registers[src as usize].clone();
                let slot = &mut frame.slots[slot as usize];
                slot.value = value;
                slot.initialized = true;
            }
            Instr::DeclareLocal { slot, kind, initialized } => {
                frame.slots[slot as usize] = RunSlot { value: Value::Undefined, initialized, kind };
            }
            Instr::InitLocal { slot, src } => {
                let value = frame.registers[src as usize].clone();
                let slot = &mut frame.slots[slot as usize];
                slot.value = value;
                slot.initialized = true;
            }
            Instr::LoadGlobal { dst, name } => {
                let name = const_string(frame.function, name)?.to_string();
                if name == "undefined" {
                    frame.registers[dst as usize] = Value::Undefined;
                } else {
                    match interp.global.borrow().lookup(&name) {
                        Lookup::Value(v) => frame.registers[dst as usize] = v,
                        Lookup::Uninitialized => {
                            return Err(VmErr::Msg(format!(
                                "ReferenceError: Cannot access '{name}' before initialization"
                            )));
                        }
                        Lookup::Missing => {
                            return Err(VmErr::Msg(format!(
                                "ReferenceError: {name} is not defined"
                            )));
                        }
                    }
                }
            }
            Instr::StoreGlobal { name, src } => {
                let name = const_string(frame.function, name)?.to_string();
                let value = frame.registers[src as usize].clone();
                interp.assign_or_set_binding(&name, value)?;
            }
            Instr::DefineGlobal { name, src, kind, initialized } => {
                let name = const_string(frame.function, name)?.to_string();
                let value = frame.registers[src as usize].clone();
                interp.declare_binding(&name, value, bind_kind(kind), initialized)?;
            }
            Instr::InitGlobal { name, src } => {
                let name = const_string(frame.function, name)?.to_string();
                let value = frame.registers[src as usize].clone();
                interp.set_binding(&name, value)?;
            }
            Instr::HoistVarGlobal { name } => {
                let name = const_string(frame.function, name)?.to_string();
                if !interp.global.borrow().has(&name) {
                    interp.declare_binding(&name, Value::Undefined, BindKind::Var, true)?;
                }
            }
            Instr::BareVarLocal { slot } => {
                if !frame.slots[slot as usize].initialized {
                    return Err(VmErr::Msg(format!(
                        "ReferenceError: Cannot access '{}' before initialization",
                        frame.function.slots[slot as usize].name
                    )));
                }
            }
            Instr::BareVarGlobal { name } => {
                let name = const_string(frame.function, name)?.to_string();
                if interp.global.borrow().get(&name).is_none() {
                    interp.assign_or_set_binding(&name, Value::Undefined)?;
                }
            }
            Instr::LoadThis { dst } => {
                frame.registers[dst as usize] = frame.this_value.clone();
            }
            Instr::LoadGlobalThis { dst } => {
                frame.registers[dst as usize] =
                    interp.global.borrow().get("this").unwrap_or(Value::Undefined);
            }
            Instr::TypeofGlobal { dst, name } => {
                let name = const_string(frame.function, name)?.to_string();
                let value = interp.global.borrow().get(&name).unwrap_or(Value::Undefined);
                frame.registers[dst as usize] = interp.un_op(crate::parser::UnOp::Typeof, &value)?;
            }
            Instr::TypeofLocal { dst, slot } => {
                let value = if frame.slots[slot as usize].initialized {
                    frame.slots[slot as usize].value.clone()
                } else {
                    Value::Undefined
                };
                frame.registers[dst as usize] = interp.un_op(crate::parser::UnOp::Typeof, &value)?;
            }
            Instr::Binary { dst, op, lhs, rhs } => {
                let l = frame.registers[lhs as usize].clone();
                let r = frame.registers[rhs as usize].clone();
                frame.registers[dst as usize] = interp.apply_binary(op, &l, &r)?;
            }
            Instr::Unary { dst, op, src } => {
                let v = frame.registers[src as usize].clone();
                frame.registers[dst as usize] = interp.un_op(op, &v)?;
            }
            Instr::CompoundLocal { dst, slot, op, rhs } => {
                let value = compound_slot(interp, frame, slot, op, rhs)?;
                frame.registers[dst as usize] = value;
            }
            Instr::CompoundGlobal { dst, name, op, rhs } => {
                let name = const_string(frame.function, name)?.to_string();
                let rhs = frame.registers[rhs as usize].clone();
                frame.registers[dst as usize] = interp.compound_assign_global(&name, op, rhs)?;
            }
            Instr::CompoundProp { dst, obj, key, op, rhs } => {
                let Some(bin) = op.bin_op() else {
                    return Err(internal("plain `=` in compound prop"));
                };
                let obj = frame.registers[obj as usize].clone();
                let key = frame.registers[key as usize].clone();
                let rhs = frame.registers[rhs as usize].clone();
                frame.registers[dst as usize] = interp.compound_assign_prop(&obj, &key, bin, rhs)?;
            }
            Instr::IncLocal { dst, slot, delta, prefix } => {
                check_slot_writable(frame, slot)?;
                let current = frame.slots[slot as usize].value.clone();
                let updated = Value::Number(if delta > 0 {
                    interp.tn(&current) + 1.0
                } else {
                    interp.tn(&current) - 1.0
                });
                frame.slots[slot as usize].value = updated.clone();
                frame.slots[slot as usize].initialized = true;
                frame.registers[dst as usize] = if prefix { updated } else { current };
            }
            Instr::IncGlobal { dst, name, delta, prefix } => {
                let name = const_string(frame.function, name)?.to_string();
                frame.registers[dst as usize] =
                    interp.inc_global_binding(&name, delta > 0, prefix)?;
            }
            Instr::IncProp { dst, obj, key, delta, prefix } => {
                let obj = frame.registers[obj as usize].clone();
                let key = frame.registers[key as usize].clone();
                frame.registers[dst as usize] =
                    interp.inc_prop_value(&obj, &key, delta > 0, prefix)?;
            }
            Instr::DelProp { dst, obj, key } => {
                let obj = frame.registers[obj as usize].clone();
                let key = frame.registers[key as usize].clone();
                frame.registers[dst as usize] = interp.delete_member(&obj, &key)?;
            }
            Instr::DelGlobal { dst, name } => {
                let name = const_string(frame.function, name)?.to_string();
                let bound = interp.global.borrow().get(&name).is_some();
                frame.registers[dst as usize] = Value::Bool(!bound);
            }
            Instr::Jump { target } => {
                frame.ip = target as usize;
            }
            Instr::JumpIfTrue { src, target } => {
                if interp.truthy(&frame.registers[src as usize]) {
                    frame.ip = target as usize;
                }
            }
            Instr::JumpIfFalse { src, target } => {
                if !interp.truthy(&frame.registers[src as usize]) {
                    frame.ip = target as usize;
                }
            }
            Instr::JumpIfNullish { src, target } => {
                if matches!(
                    frame.registers[src as usize],
                    Value::Null | Value::Undefined
                ) {
                    frame.ip = target as usize;
                }
            }
            Instr::JumpIfNotNullish { src, target } => {
                if !matches!(
                    frame.registers[src as usize],
                    Value::Null | Value::Undefined
                ) {
                    frame.ip = target as usize;
                }
            }
            Instr::LoopHead => {
                interp.consume_loop()?;
            }
            // `return` signals through `Ret`, like the evaluator's bodies:
            // `call_this` maps it to a value, `ctor` maps object returns to
            // the returned object. Only falling off the end yields `Ok`.
            Instr::Return { src } => {
                return Err(VmErr::Ret(frame.registers[src as usize].clone()));
            }
            Instr::ReturnUndefined => {
                return Err(VmErr::Ret(Value::Undefined));
            }
            Instr::Throw { src } => {
                return Err(VmErr::Throw(frame.registers[src as usize].clone()));
            }
            Instr::GetProp { dst, obj, key } => {
                let obj = frame.registers[obj as usize].clone();
                let key = frame.registers[key as usize].clone();
                frame.registers[dst as usize] = interp.get_prop_value(&obj, &key)?;
            }
            Instr::SetProp { obj, key, val } => {
                let obj = frame.registers[obj as usize].clone();
                let key = frame.registers[key as usize].clone();
                let val = frame.registers[val as usize].clone();
                interp.assign_member(&obj, &key, val)?;
            }
            Instr::Call { dst, callee, args, argc } => {
                let argv = take_range(frame, args, argc)?;
                let callee = frame.registers[callee as usize].clone();
                frame.registers[dst as usize] = interp.call_this(&callee, Value::Undefined, argv)?;
            }
            Instr::CallMethod { dst, callee, this, args, argc } => {
                let argv = take_range(frame, args, argc)?;
                let callee = frame.registers[callee as usize].clone();
                let this = frame.registers[this as usize].clone();
                frame.registers[dst as usize] = interp.call_this(&callee, this, argv)?;
            }
            Instr::Construct { dst, callee, args, argc } => {
                let argv = take_range(frame, args, argc)?;
                let callee = frame.registers[callee as usize].clone();
                frame.registers[dst as usize] = interp.ctor(&callee, argv)?;
            }
            Instr::CallSpread { dst, callee, tmpl } => {
                let template = spread_template(frame, tmpl)?;
                let argv = spread_argv(frame, &template)?;
                let callee = frame.registers[callee as usize].clone();
                frame.registers[dst as usize] = interp.call_this(&callee, Value::Undefined, argv)?;
            }
            Instr::MethodSpread { dst, callee, this, tmpl } => {
                let template = spread_template(frame, tmpl)?;
                let argv = spread_argv(frame, &template)?;
                let callee = frame.registers[callee as usize].clone();
                let this = frame.registers[this as usize].clone();
                frame.registers[dst as usize] = interp.call_this(&callee, this, argv)?;
            }
            Instr::BuildArray { dst, tmpl } => {
                let template = spread_template(frame, tmpl)?;
                frame.registers[dst as usize] = spread_array(interp, frame, &template)?;
            }
            Instr::ToDestructArray { dst, src } => {
                frame.registers[dst as usize] =
                    to_destruct_array(&frame.registers[src as usize])?;
            }
            Instr::RestArray { dst, src, from } => {
                let rest = match &frame.registers[src as usize] {
                    Value::Array(items) => {
                        items.borrow().get(from as usize..).unwrap_or(&[]).to_vec()
                    }
                    _ => return Err(internal("rest of non-array")),
                };
                frame.registers[dst as usize] = Value::array(rest);
            }
            Instr::CheckDestructObject { dst, src } => {
                frame.registers[dst as usize] =
                    check_destruct_object(&frame.registers[src as usize])?;
            }
            Instr::RestObject { dst, src, keys, taken } => {
                let source = frame.registers[src as usize].clone();
                let keys = frame.registers[keys as usize].clone();
                let taken = frame.registers[taken as usize].clone();
                frame.registers[dst as usize] = rest_object(interp, &source, &keys, &taken)?;
            }
            Instr::NewObject { .. } | Instr::SetOwnProp { .. } => {
                // Superseded by `BuildObject`; retained as valid IR, never
                // emitted. Reaching here is a compiler bug.
                return Err(internal("incremental object construction is retired"));
            }
            Instr::NormalKey { dst, src } => {
                let key = match &frame.registers[src as usize] {
                    Value::String(s) => Value::String(s.clone()),
                    Value::Number(n) => Value::String(n.to_string()),
                    Value::Symbol(s) => Value::String(symbol_slot_key(s)),
                    _ => Value::Undefined,
                };
                frame.registers[dst as usize] = key;
            }
            Instr::LoadGlobalSoft { dst, name } => {
                let name = const_string(frame.function, name)?;
                frame.registers[dst as usize] =
                    interp.global.borrow().get(name).unwrap_or(Value::Undefined);
            }
            Instr::LoadLocalSoft { dst, slot } => {
                frame.registers[dst as usize] = if frame.slots[slot as usize].initialized {
                    frame.slots[slot as usize].value.clone()
                } else {
                    Value::Undefined
                };
            }
            Instr::BuildObject { dst, tmpl } => {
                let template = match &frame.function.constants[tmpl as usize] {
                    Constant::ObjectTemplate(entries) => entries.clone(),
                    _ => return Err(internal("bad object template")),
                };
                frame.registers[dst as usize] = build_object(interp, frame, &template)?;
            }
            Instr::NewArray { dst, args, argc } => {
                let items = take_range(frame, args, argc)?;
                frame.registers[dst as usize] = Value::checked_array(items)?;
            }
            Instr::MakeFunction { dst, func } => {
                let code = match &frame.function.constants[func as usize] {
                    Constant::Function(code) => code.clone(),
                    _ => return Err(internal("bad function constant")),
                };
                frame.registers[dst as usize] =
                    make_function(interp, &code, interp.global.clone(), None);
            }
            Instr::MakeAstFunction { dst, ast } => {
                let ast = match &frame.function.constants[ast as usize] {
                    Constant::AstFunction(ast) => ast.clone(),
                    _ => return Err(internal("bad ast-function constant")),
                };
                frame.registers[dst as usize] =
                    make_ast_function(interp, &ast, interp.global.clone(), None);
            }
            Instr::Template { dst, quasis, args, argc } => {
                let quasis = match &frame.function.constants[quasis as usize] {
                    Constant::StringList(quasis) => quasis.clone(),
                    _ => return Err(internal("bad template constant")),
                };
                let values = take_range(frame, args, argc)?;
                frame.registers[dst as usize] = interp.render_template(&quasis, &values)?;
            }
            Instr::EnumKeys { dst, src } => {
                let source = frame.registers[src as usize].clone();
                let keys = interp.keys_with_proxy_trap(&source)?;
                frame.registers[dst as usize] =
                    Value::array(keys.into_iter().map(Value::String).collect());
            }
            Instr::ForOfInit { iter, next, src } => {
                let source = frame.registers[src as usize].clone();
                let iterator = interp.iterator_for(&source)?;
                let next_fn = interp.prop(&iterator, &Value::String("next".to_string()))?;
                if matches!(next_fn, Value::Undefined) {
                    return Err(VmErr::Msg("iterator has no next() method".to_string()));
                }
                frame.registers[iter as usize] = iterator;
                frame.registers[next as usize] = next_fn;
            }
            Instr::IterNext { done, value, iter, next } => {
                let iterator = frame.registers[iter as usize].clone();
                let next_fn = frame.registers[next as usize].clone();
                let result = interp.call_this(&next_fn, iterator, vec![])?;
                let finished = result
                    .get_prop("done")
                    .map(|flag| flag.is_truthy())
                    .unwrap_or(true);
                frame.registers[done as usize] = Value::Bool(finished);
                frame.registers[value as usize] =
                    result.get_prop("value").unwrap_or(Value::Undefined);
            }
            Instr::CloseIterator { src } => {
                crate::interpreter::close_iterator(&frame.registers[src as usize]);
            }
            Instr::PushCatch { target, dst } => {
                frame.handlers.push(HandlerEntry { target, dst, catch_returns: false });
            }
            Instr::PushFinally { target, dst } => {
                frame.handlers.push(HandlerEntry { target, dst, catch_returns: true });
            }
            Instr::PopHandler => {
                if frame.handlers.pop().is_none() {
                    return Err(internal("handler stack underflow"));
                }
            }
            Instr::Rethrow => match frame.pending.take() {
                Some(error) => return Err(error),
                None => return Err(internal("rethrow without a pending error")),
            },
            Instr::SuperMember { dst, key } => {
                let proto = interp.global.borrow().get(crate::interpreter::SUPER_PROTO);
                let Some(proto) = proto else {
                    return Err(VmErr::Msg("'super' used outside a derived class".to_string()));
                };
                let key = frame.registers[key as usize].clone();
                frame.registers[dst as usize] = interp.get_prop_value(&proto, &key)?;
            }
            Instr::SuperCall { dst, args, argc } => {
                let argv = take_range(frame, args, argc)?;
                frame.registers[dst as usize] = super_call(interp, argv)?;
            }
            Instr::SuperCallSpread { dst, tmpl } => {
                let template = spread_template(frame, tmpl)?;
                let argv = spread_argv(frame, &template)?;
                frame.registers[dst as usize] = super_call(interp, argv)?;
            }
            Instr::Raise { msg } => {
                let message = const_string(frame.function, msg)?.to_string();
                return Err(VmErr::Msg(message));
            }
            Instr::BuildClass { dst, tmpl } => {
                let template = match &frame.function.constants[tmpl as usize] {
                    Constant::ClassTemplate(template) => template.clone(),
                    _ => return Err(internal("bad class template")),
                };
                frame.registers[dst as usize] = build_class_from_template(interp, frame, &template)?;
            }
            Instr::PropertyKey { dst, src } => {
                let key = frame.registers[src as usize].clone();
                let key = interp.property_key(&key)?;
                frame.registers[dst as usize] = Value::String(key);
            }
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            land_handler(interp, frame, error)?;
        }
    }
}

/// Route an instruction error to the innermost handler that takes it, or
/// re-raise. Mirrors the evaluator's `try`: only thrown values and runtime
/// errors land on a catch; `return` unwinds past catches to the nearest
/// finally (or out); abandonment bypasses every handler.
fn land_handler(
    interp: &mut Interpreter,
    frame: &mut CallFrame,
    error: VmErr,
) -> Result<(), VmErr> {
    if error.is_abandon() {
        return Err(error);
    }
    let catchable = matches!(
        error,
        VmErr::Throw(_) | VmErr::Msg(_) | VmErr::RuntimeError(_) | VmErr::Ret(_)
    );
    if !catchable {
        return Err(error);
    }
    let mut entry = match frame.handlers.pop() {
        Some(entry) => entry,
        None => return Err(error),
    };
    while matches!(error, VmErr::Ret(_)) && !entry.catch_returns {
        entry = match frame.handlers.pop() {
            Some(entry) => entry,
            None => return Err(error),
        };
    }
    let value = match &error {
        VmErr::Throw(value) => value.clone(),
        VmErr::Msg(message) => crate::error::error_value_with_stack(message, interp.get_stack()),
        VmErr::RuntimeError(data) => {
            crate::error::error_value_with_stack(&data.message, &data.stack)
        }
        VmErr::Ret(_) => Value::Undefined,
        _ => unreachable!("filtered above"),
    };
    frame.registers[entry.dst as usize] = value;
    frame.pending = Some(error);
    frame.ip = entry.target as usize;
    Ok(())
}

fn bind_kind(kind: SlotKind) -> BindKind {
    match kind {
        SlotKind::Var => BindKind::Var,
        SlotKind::Let => BindKind::Let,
        SlotKind::Const => BindKind::Const,
    }
}

/// Mirror of [`Environment::assign`]'s refusal rules for one slot: const
/// reassignment and dead-zone writes fail before any read or coercion.
fn check_slot_writable(frame: &CallFrame, slot: Slot) -> Result<(), VmErr> {
    let slot_info = &frame.slots[slot as usize];
    let name = &frame.function.slots[slot as usize].name;
    if slot_info.kind == SlotKind::Const && slot_info.initialized {
        return Err(VmErr::Msg(format!(
            "TypeError: Assignment to constant variable '{name}'"
        )));
    }
    if !slot_info.initialized {
        return Err(VmErr::Msg(format!(
            "ReferenceError: Cannot access '{name}' before initialization"
        )));
    }
    Ok(())
}

/// Read-modify-write on a slot with the evaluator's exact order: the `+`
/// RHS coercion runs before the writability check, the current value's
/// coercion (when it needs one) before the operator, and the write last.
/// Nothing is written when the operator fails.
fn compound_slot(
    interp: &mut Interpreter,
    frame: &mut CallFrame,
    slot: Slot,
    op: crate::parser::AssignOp,
    rhs: Reg,
) -> Result<Value, VmErr> {
    let Some(bin) = op.bin_op() else {
        return Err(internal("plain `=` in compound slot"));
    };
    let rhs = frame.registers[rhs as usize].clone();
    let rhs = if matches!(op, crate::parser::AssignOp::Add) {
        interp.coerce_for_concat(&rhs)?
    } else {
        rhs
    };
    check_slot_writable(frame, slot)?;
    let mut current = frame.slots[slot as usize].value.clone();
    if matches!(bin, crate::parser::BinOp::Add) && Interpreter::needs_concat_coercion(&current) {
        current = interp.coerce_for_concat(&current)?;
    }
    let combined = interp.bin_op(bin, &current, &rhs)?;
    frame.slots[slot as usize].value = combined.clone();
    frame.slots[slot as usize].initialized = true;
    Ok(combined)
}

/// Instantiate one method/accessor/constructor function from a class
/// template's function constant, closing over `closure`.
fn class_function(
    interp: &Interpreter,
    frame: &CallFrame,
    index: u16,
    closure: Env,
    name_override: Option<Rc<str>>,
) -> Result<Value, VmErr> {
    match frame.function.constants.get(index as usize) {
        Some(Constant::Function(code)) => {
            Ok(make_function(interp, code, closure, name_override))
        }
        Some(Constant::AstFunction(ast)) => {
            Ok(make_ast_function(interp, ast, closure, name_override))
        }
        _ => Err(internal("bad class function constant")),
    }
}

/// Build a class value from a template: resolve member names, instantiate
/// functions over the superclass scopes, gather members exactly like the
/// evaluator's member walk, then share its assembly.
fn build_class_from_template(
    interp: &mut Interpreter,
    frame: &CallFrame,
    template: &super::constants::ClassTemplate,
) -> Result<Value, VmErr> {
    use super::constants::{ClassMemberKind, ClassNameTemplate};
    use crate::interpreter::{ClassAssembly, insert_class_accessor};

    let def_scope = match &template.expr_name {
        Some(_) => Rc::new(RefCell::new(Environment::child(interp.global.clone()))),
        None => interp.global.clone(),
    };
    let super_cls = template
        .superclass
        .map(|reg| frame.registers[reg as usize].clone());
    let super_proto = interp.super_proto_for(&super_cls)?;
    let member_closure = Interpreter::member_closure_env(&def_scope, &super_proto);

    let mut proto_props = Vec::new();
    let mut statics = vec![(
        "name".to_string(),
        Value::String(template.name.clone()),
    )];
    let mut static_attrs = vec![(
        "name".to_owned(),
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable: true,
        },
    )];
    let mut static_has_accessors = false;
    for member in &template.members {
        let computed;
        let key = match &member.name {
            ClassNameTemplate::Static(name) => name.clone(),
            ClassNameTemplate::Computed(reg) => {
                computed = frame.registers[*reg as usize].clone();
                interp.property_key(&computed)?
            }
        };
        // Computed names are known only now; static ones were set when
        // each function compiled.
        let display = |prefix: &str| match &member.name {
            ClassNameTemplate::Static(_) => None,
            ClassNameTemplate::Computed(_) => Some(Rc::from(format!("{prefix}{key}"))),
        };
        match member.kind {
            ClassMemberKind::Method => {
                let func = member.func.ok_or_else(|| internal("method without function"))?;
                let fn_val =
                    class_function(interp, frame, func, member_closure.clone(), display(""))?;
                if member.is_static {
                    statics.push((key.clone(), fn_val));
                    static_attrs.push((
                        key,
                        PropAttrs {
                            writable: true,
                            enumerable: false,
                            configurable: true,
                        },
                    ));
                } else {
                    proto_props.push((key, fn_val));
                }
            }
            ClassMemberKind::Getter | ClassMemberKind::Setter => {
                let func = member.func.ok_or_else(|| internal("accessor without function"))?;
                let prefix = if member.kind == ClassMemberKind::Getter { "get " } else { "set " };
                let fn_val =
                    class_function(interp, frame, func, member_closure.clone(), display(prefix))?;
                if member.is_static {
                    insert_class_accessor(&mut statics, &key, fn_val);
                    static_attrs.push((
                        key,
                        PropAttrs {
                            writable: false,
                            enumerable: false,
                            configurable: true,
                        },
                    ));
                    static_has_accessors = true;
                } else {
                    proto_props.push((key, fn_val));
                }
            }
            ClassMemberKind::Field => {
                let reg = member.value.ok_or_else(|| internal("field without value"))?;
                let value = frame.registers[reg as usize].clone();
                statics.push((key.clone(), value));
                static_attrs.push((key, PropAttrs::default()));
            }
        }
    }

    let super_ctor_value = Interpreter::super_ctor_for(&super_cls);
    let ctor_closure = match (&super_ctor_value, template.ctor_computed_keys.is_empty()) {
        (None, true) => def_scope.clone(),
        _ => {
            let env = Rc::new(RefCell::new(Environment::child(def_scope.clone())));
            if let Some(target) = super_ctor_value {
                env.borrow_mut().set("__super_ctor", target);
            }
            for (index, reg) in template.ctor_computed_keys.iter().enumerate() {
                let key = frame.registers[*reg as usize].clone();
                env.borrow_mut()
                    .set(&super::constants::class_key_name(index), key);
            }
            env
        }
    };
    let constructor = class_function(interp, frame, template.ctor_func, ctor_closure, None)?;
    let mut static_blocks = Vec::with_capacity(template.blocks.len());
    for block in &template.blocks {
        match frame.function.constants.get(*block as usize) {
            Some(Constant::AstFunction(ast)) => static_blocks.push(ast.body.clone()),
            _ => return Err(internal("bad class static block")),
        }
    }
    let class_val = interp.assemble_class(ClassAssembly {
        name: template.name.clone(),
        super_cls,
        super_proto,
        constructor,
        constructor_length: template.ctor_length,
        proto_props,
        statics,
        static_attrs,
        static_has_accessors,
        static_blocks,
    })?;
    if let Some(name) = &template.expr_name {
        def_scope.borrow_mut().set(name, class_val.clone());
    }
    Ok(class_val)
}

/// `super(...)`: invoke the superclass constructor on the current `this`.
fn super_call(interp: &mut Interpreter, argv: Vec<Value>) -> Result<Value, VmErr> {
    let scope = interp.global.borrow();
    let this_val = scope.get("this").unwrap_or(Value::Undefined);
    let super_ctor = scope.get("__super_ctor").ok_or_else(|| {
        VmErr::Msg("super used outside a derived class".to_string())
    })?;
    drop(scope);
    interp.invoke_ctor(&super_ctor, this_val, argv)
}

/// Clone one verified operand range out of the register file.
fn take_range(frame: &CallFrame, start: Reg, count: u16) -> Result<Vec<Value>, VmErr> {
    let start = start as usize;
    let end = start.saturating_add(count as usize);
    match frame.registers.get(start..end) {
        Some(range) => Ok(range.to_vec()),
        None => Err(internal("operand range out of bounds")),
    }
}

/// Build one object literal from its template and evaluated registers.
/// Insertion (including accessor pairing and dedup order), spread, symbol
/// registration, and the property-count limit mirror the evaluator's
/// construction entry for entry.
fn build_object(
    interp: &mut Interpreter,
    frame: &CallFrame,
    template: &[PropEntry],
) -> Result<Value, VmErr> {
    let mut object = Vec::new();
    let mut positions: HashMap<String, Vec<usize>> = HashMap::new();
    let mut accessors = HashMap::new();
    let mut symbol_keys = Vec::new();
    for entry in template {
        if entry.kind == PropKind::Spread {
            let src = frame.registers[entry.val as usize].clone();
            interp.for_each_spread_entry(&src, |key, value| {
                insert_object_property(
                    &mut object,
                    &mut positions,
                    &mut accessors,
                    key,
                    value,
                    None,
                );
                check_prop_limit(&positions)
            })?;
        } else {
            let key_src = entry.key.ok_or_else(|| internal("spread-shaped data entry"))?;
            let key = match key_src {
                KeySrc::Const(index) => const_string(frame.function, index)?.to_string(),
                KeySrc::Reg(reg) => match &frame.registers[reg as usize] {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Symbol(s) => {
                        let key = symbol_slot_key(s);
                        symbol_keys.push((key.clone(), s.clone()));
                        key
                    }
                    // The compiler skips evaluating the value for these;
                    // hitting one here means a hand-built unit.
                    _ => continue,
                },
            };
            let value = frame.registers[entry.val as usize].clone();
            let kind = match entry.kind {
                PropKind::Data => None,
                PropKind::Getter => Some(ObjectAccessorKind::Getter),
                PropKind::Setter => Some(ObjectAccessorKind::Setter),
                PropKind::Spread => return Err(internal("misrouted spread entry")),
            };
            insert_object_property(&mut object, &mut positions, &mut accessors, key, value, kind);
        }
        if positions.len() > crate::value::MAX_OBJECT_PROPS {
            return Err(crate::value::limit_err(
                "Maximum object property count exceeded",
            ));
        }
    }
    let result = Value::checked_object(object.into_iter().flatten().collect())?;
    if let Value::Object { props } = &result {
        let mut meta = props.meta.borrow_mut();
        meta.has_accessors = !accessors.is_empty();
        for (key, symbol) in symbol_keys {
            meta.set_symbol_key(&key, symbol);
        }
    }
    Ok(result)
}

fn spread_template(frame: &CallFrame, tmpl: u16) -> Result<Vec<SpreadEntry>, VmErr> {
    match frame.function.constants.get(tmpl as usize) {
        Some(Constant::SpreadTemplate(entries)) => Ok(entries.clone()),
        _ => Err(internal("bad spread template")),
    }
}

/// Build a call argument list with call-spread rules: arrays splice,
/// anything else passes as one argument. Mirrors the evaluator's
/// argument loop, including the count limit.
fn spread_argv(frame: &CallFrame, template: &[SpreadEntry]) -> Result<Vec<Value>, VmErr> {
    let mut argv = Vec::new();
    for entry in template {
        let value = frame.registers[entry.reg as usize].clone();
        if !entry.spread {
            push_call_arg(&mut argv, value)?;
            continue;
        }
        match &value {
            Value::Array(arr) => {
                let items = arr.borrow();
                if argv.len().saturating_add(items.len()) > crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum argument count exceeded"));
                }
                argv.extend(items.iter().cloned());
            }
            _ => push_call_arg(&mut argv, value)?,
        }
    }
    Ok(argv)
}

/// Build an array literal with element-spread rules: arrays splice, strings
/// spread per character, anything else drains the iterator protocol.
/// Mirrors the evaluator's element loop, including every limit check.
fn spread_array(
    interp: &mut Interpreter,
    frame: &CallFrame,
    template: &[SpreadEntry],
) -> Result<Value, VmErr> {
    let mut items = Vec::new();
    for entry in template {
        let value = frame.registers[entry.reg as usize].clone();
        if !entry.spread {
            items.push(value);
        } else {
            match &value {
                Value::Array(arr) => {
                    let elements = arr.borrow();
                    if items.len().saturating_add(elements.len()) > crate::value::MAX_ARRAY_LEN {
                        return Err(crate::value::limit_err("Maximum array length exceeded"));
                    }
                    items.extend(elements.iter().cloned());
                }
                Value::String(s) => {
                    if items.len().saturating_add(s.chars().count()) > crate::value::MAX_ARRAY_LEN
                    {
                        return Err(crate::value::limit_err("Maximum array length exceeded"));
                    }
                    items.extend(s.chars().map(|c| Value::String(c.to_string())));
                }
                other => {
                    let drained = interp.drain_iterable(other)?;
                    if items.len().saturating_add(drained.len()) > crate::value::MAX_ARRAY_LEN {
                        return Err(crate::value::limit_err("Maximum array length exceeded"));
                    }
                    items.extend(drained);
                }
            }
        }
        if items.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
    }
    Value::checked_array(items)
}

/// Materialize one value for an array destructuring pattern: arrays
/// clone, strings split per character (length-checked), and anything else
/// — plain objects included, which must never become a sparse vector keyed
/// by guest data — becomes `[]`. Mirrors the evaluator's pattern path.
fn to_destruct_array(value: &Value) -> Result<Value, VmErr> {
    match value {
        Value::Array(arr) => Ok(Value::array(arr.borrow().clone())),
        Value::Object { .. } => Ok(Value::array(Vec::new())),
        Value::String(s) => {
            if s.chars().count() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            Ok(Value::array(
                s.chars().map(|c| Value::String(c.to_string())).collect(),
            ))
        }
        _ => Ok(Value::array(Vec::new())),
    }
}

/// Reject a nullish object-pattern source, else snapshot the own enumerable
/// string keys a later `{ ...rest }` draws from. Only objects contribute
/// keys; every other non-nullish value destructures through `Get` alone.
fn check_destruct_object(value: &Value) -> Result<Value, VmErr> {
    if matches!(value, Value::Null | Value::Undefined) {
        return Err(VmErr::Msg(format!(
            "TypeError: Cannot destructure properties of {}",
            if matches!(value, Value::Null) {
                "null"
            } else {
                "undefined"
            }
        )));
    }
    let keys: Vec<Value> = match value {
        Value::Object { props: oprops, .. } => {
            let slots = oprops.borrow();
            let meta = oprops.meta.borrow();
            slots
                .iter()
                .filter(|(k, _)| {
                    meta.attrs_of(k).enumerable && !crate::interpreter::is_internal_key(k)
                })
                .map(|(k, _)| Value::String(k.clone()))
                .collect()
        }
        _ => Vec::new(),
    };
    Ok(Value::array(keys))
}

/// `{ ...rest }`: the snapshotted `keys` minus the `taken` keys, each read
/// through the normal member path (getters run, the prototype applies).
fn rest_object(
    interp: &mut Interpreter,
    source: &Value,
    keys: &Value,
    taken: &Value,
) -> Result<Value, VmErr> {
    let as_strings = |list: &Value| match list {
        Value::Array(items) => items
            .borrow()
            .iter()
            .map(|key| interp.property_key(key))
            .collect::<Result<Vec<_>, _>>(),
        _ => Err(internal("rest key list of non-array")),
    };
    let taken = as_strings(taken)?;
    let mut remaining = Vec::new();
    for key in as_strings(keys)? {
        if taken.iter().any(|t| t == &key) {
            continue;
        }
        let value = interp.get_prop_value(source, &Value::String(key.clone()))?;
        remaining.push((key, value));
    }
    Value::checked_object(remaining)
}

fn check_prop_limit(positions: &HashMap<String, Vec<usize>>) -> Result<(), VmErr> {
    if positions.len() > crate::value::MAX_OBJECT_PROPS {
        return Err(crate::value::limit_err(
            "Maximum object property count exceeded",
        ));
    }
    Ok(())
}

/// Instantiate a bytecode-backed function value. The AST body is a fresh
/// empty program per instantiation: unique `Rc` identity per object (the
/// callback registry keys on it), never executed (calls dispatch on
/// `bytecode`, generators and async never compile).
fn make_function(
    interp: &Interpreter,
    code: &Rc<BytecodeFunction>,
    closure: Env,
    name_override: Option<Rc<str>>,
) -> Value {
    let params: Vec<String> = code.slots[..code.parameter_count as usize]
        .iter()
        .map(|slot| slot.name.clone())
        .collect();
    Value::Function(Rc::new(FunctionData {
        identity: Rc::new(0),
        name: name_override.or_else(|| code.name.as_deref().map(Rc::from)),
        properties: FunctionData::properties_with_default_prototype(&interp.persistent_global),
        standard_properties_initialized: Rc::new(Cell::new(false)),
        params: intern_params(&params),
        body: Rc::new(Vec::new()),
        // Lexical, like the evaluator: the defining frame environment.
        // `None` would resolve free variables through the *caller's* frame
        // (dynamic scope). Capture-free-ness only means no *slot* bindings
        // escape — slot bindings are invisible to environment chains, which
        // is why capturing functions decline compilation.
        closure: Some(closure),
        is_arrow: code.is_arrow,
        is_constructor: code.is_constructor,
        is_async: false,
        is_generator: false,
        uses_arguments: false,
        bound: None,
        bytecode: Some(code.clone()),
    }))
}

/// Instantiate a per-function AST fallback: the evaluator's own function
/// shape, closing over the defining frame environment. Capture-free by
/// construction (capturing functions decline the whole unit), but the link
/// is still load-bearing: without it free variables would resolve through
/// the caller's frame instead of the definition scope.
fn make_ast_function(
    interp: &Interpreter,
    ast: &super::constants::AstFunction,
    closure: Env,
    name_override: Option<Rc<str>>,
) -> Value {
    Value::Function(Rc::new(FunctionData {
        identity: Rc::new(0),
        name: name_override.or_else(|| ast.name.as_deref().map(Rc::from)),
        properties: FunctionData::properties_with_default_prototype(&interp.persistent_global),
        standard_properties_initialized: Rc::new(Cell::new(false)),
        params: intern_params(&ast.params),
        body: ast.body.clone(),
        closure: Some(closure),
        is_arrow: ast.is_arrow,
        is_constructor: ast.is_constructor,
        is_async: ast.is_async,
        is_generator: ast.is_generator,
        uses_arguments: ast.uses_arguments,
        bound: None,
        bytecode: None,
    }))
}
