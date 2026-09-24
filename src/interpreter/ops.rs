use std::rc::Rc;

use super::Interpreter;
use crate::error::VmErr;
use crate::parser::{BinOp, UnOp};
use crate::value::Value;

/// `===`.
///
/// Primitives compare by value; everything else compares by *reference
/// identity*, which is what makes `o === o` true and `{} === {}` false. Each
/// reference type is identified by the address of the allocation its clones
/// share, so two `Value`s naming the same object agree.
/// The numeric value a `BigInt` may be compared against, or `None` for a
/// value that has no numeric comparison at all.
fn numeric_comparand(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.trim().parse().ok(),
        Value::Null => Some(0.0),
        _ => None,
    }
}

pub fn strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) | (Value::Undefined, Value::Undefined) => true,
        // The global aliases all denote the one global scope.
        (Value::GlobalObject, Value::GlobalObject) => true,
        (Value::Object { props: x }, Value::Object { props: y }) => Rc::ptr_eq(x, y),
        (Value::Proxy(x), Value::Proxy(y)) => Rc::ptr_eq(x, y),
        (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
        (Value::Promise(x), Value::Promise(y)) => Rc::ptr_eq(x, y),
        (Value::Generator { inner: x }, Value::Generator { inner: y }) => Rc::ptr_eq(x, y),
        (Value::StringIterator { inner: x }, Value::StringIterator { inner: y }) => {
            Rc::ptr_eq(x, y)
        }
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(&x.prototype, &y.prototype),
        (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(&x.identity, &y.identity),
        (Value::NativeFunction { callable: x, .. }, Value::NativeFunction { callable: y, .. }) => {
            std::ptr::fn_addr_eq(*x, *y)
        }
        (Value::HostFunction { properties: x, .. }, Value::HostFunction { properties: y, .. }) => {
            x.meta.borrow().host_function_id == y.meta.borrow().host_function_id
        }
        (Value::HostPending { id: x }, Value::HostPending { id: y }) => x == y,
        (Value::Symbol(x), Value::Symbol(y)) => x.id == y.id,
        (Value::BigInt(x), Value::BigInt(y)) => x.compare(y).is_eq(),
        (Value::Date(x), Value::Date(y)) => Rc::ptr_eq(x, y),
        (Value::SharedArrayBuffer(x), Value::SharedArrayBuffer(y)) => x.identity() == y.identity(),
        (Value::Error(x), Value::Error(y)) => Rc::ptr_eq(&x.identity, &y.identity),
        _ => false,
    }
}

/// Property slots the VM uses to store symbol-keyed values are named with a
/// reserved `__symbol…__` prefix. They are real slots so the prototype walk
/// finds them, but they must stay out of `Object.keys`, `for…in` and
/// `JSON.stringify`, which enumerate string keys only.
pub fn is_internal_key(key: &str) -> bool {
    // A private class member (`#x`) is stored as an ordinary slot named `#x`.
    // Nothing outside the class body can write that name, and it must not
    // appear in `Object.keys`, `for…in` or `JSON.stringify`.
    key.starts_with('#') || key.starts_with("__symbol") || key.starts_with("__setter:")
}

/// The slot name backing a symbol-keyed property.
///
/// Keyed by the symbol's *id*, so two symbols sharing a description still get
/// separate slots. `Symbol.iterator` keeps a fixed, readable name because the
/// evaluator looks that slot up directly when starting a `for…of`.
pub fn symbol_slot_key(symbol: &crate::value::SymbolData) -> String {
    if symbol.id == 1 {
        SYMBOL_ITERATOR_SLOT.to_string()
    } else {
        format!("__symbol:{}__", symbol.id)
    }
}

/// Recover the symbol id encoded in an object's internal symbol-key slot.
pub fn symbol_id_from_slot(key: &str) -> Option<u64> {
    if key == SYMBOL_ITERATOR_SLOT {
        return Some(1);
    }
    key.strip_prefix("__symbol:")?
        .strip_suffix("__")?
        .parse()
        .ok()
}

/// The slot every iterable stores its `[Symbol.iterator]` method in.
pub const SYMBOL_ITERATOR_SLOT: &str = "__symbol_iterator__";

impl Interpreter {
    pub fn bin_op(&self, op: BinOp, l: &Value, r: &Value) -> Result<Value, VmErr> {
        // Fast path: when both operands are already numbers, the arithmetic and
        // comparison operators need no coercion and `+` cannot be string
        // concatenation. This skips the `to_number` dispatch and the string
        // check on the hottest interpreter path.
        if let (Value::Number(a), Value::Number(b)) = (l, r) {
            let fast = match op {
                BinOp::Add => Some(Value::Number(a + b)),
                BinOp::Sub => Some(Value::Number(a - b)),
                BinOp::Mul => Some(Value::Number(a * b)),
                BinOp::Div => Some(Value::Number(a / b)),
                BinOp::Mod => Some(Value::Number(a % b)),
                BinOp::Pow => Some(Value::Number(a.powf(*b))),
                BinOp::BitAnd => Some(Value::Number(((*a as i32) & (*b as i32)) as f64)),
                BinOp::BitOr => Some(Value::Number(((*a as i32) | (*b as i32)) as f64)),
                BinOp::BitXor => Some(Value::Number(((*a as i32) ^ (*b as i32)) as f64)),
                BinOp::Shl => Some(Value::Number(((*a as i32) << ((*b as i32) & 31)) as f64)),
                BinOp::Shr => Some(Value::Number(((*a as i32) >> ((*b as i32) & 31)) as f64)),
                BinOp::UShr => Some(Value::Number(
                    ((*a as i32 as u32) >> (*b as i32 as u32 & 31)) as f64,
                )),
                BinOp::Lt => Some(Value::Bool(a < b)),
                BinOp::Gt => Some(Value::Bool(a > b)),
                BinOp::Le => Some(Value::Bool(a <= b)),
                BinOp::Ge => Some(Value::Bool(a >= b)),
                BinOp::Eq | BinOp::Seq => Some(Value::Bool(a == b)),
                BinOp::Neq | BinOp::Sneq => Some(Value::Bool(a != b)),
                _ => None,
            };
            if let Some(v) = fast {
                return Ok(v);
            }
        }
        // `BigInt` is a distinct numeric type. Arithmetic between the two
        // kinds is a `TypeError` rather than a silent coercion, because
        // narrowing to `f64` would lose the precision `BigInt` exists for.
        // Comparison and `+` with a string are the exceptions the language
        // makes, and are handled inside `bigint_op`.
        if (matches!(l, Value::BigInt(_)) || matches!(r, Value::BigInt(_)))
            && let Some(result) = self.bigint_op(op, l, r)?
        {
            return Ok(result);
        }
        Ok(match op {
            BinOp::Add => {
                // String concatenation if either side is a string; otherwise
                // numeric addition (booleans/null/etc. coerce via to_number).
                // The string side is pushed directly instead of round-tripping
                // through `vs` (which would clone it).
                // Unbounded string growth (`s = s + s` in a loop) would
                // exhaust host memory and abort the process; cap the result
                // and fail with a catchable RangeError instead.
                use crate::value::MAX_STRING_LEN;
                match (l, r) {
                    (Value::String(a), Value::String(b)) => {
                        if a.len().saturating_add(b.len()) > MAX_STRING_LEN {
                            return Err(crate::value::limit_err("Maximum string length exceeded"));
                        }
                        let mut s = String::with_capacity(a.len() + b.len());
                        s.push_str(a);
                        s.push_str(b);
                        Value::checked_string(s)?
                    }
                    (Value::String(a), _) => {
                        let rb = self.vs(r)?;
                        if a.len().saturating_add(rb.len()) > MAX_STRING_LEN {
                            return Err(crate::value::limit_err("Maximum string length exceeded"));
                        }
                        let mut s = String::with_capacity(a.len() + rb.len());
                        s.push_str(a);
                        s.push_str(&rb);
                        Value::checked_string(s)?
                    }
                    (_, Value::String(b)) => {
                        let lb = self.vs(l)?;
                        if lb.len().saturating_add(b.len()) > MAX_STRING_LEN {
                            return Err(crate::value::limit_err("Maximum string length exceeded"));
                        }
                        let mut s = String::with_capacity(lb.len() + b.len());
                        s.push_str(&lb);
                        s.push_str(b);
                        Value::checked_string(s)?
                    }
                    _ => Value::Number(self.tn(l) + self.tn(r)),
                }
            }
            BinOp::Sub => Value::Number(self.tn(l) - self.tn(r)),
            BinOp::Mul => Value::Number(self.tn(l) * self.tn(r)),
            BinOp::Div => Value::Number(self.tn(l) / self.tn(r)),
            BinOp::Mod => Value::Number(self.tn(l) % self.tn(r)),
            BinOp::Pow => Value::Number(self.tn(l).powf(self.tn(r))),
            BinOp::BitAnd => Value::Number(((self.tn(l) as i32) & (self.tn(r) as i32)) as f64),
            BinOp::BitOr => Value::Number(((self.tn(l) as i32) | (self.tn(r) as i32)) as f64),
            BinOp::BitXor => Value::Number(((self.tn(l) as i32) ^ (self.tn(r) as i32)) as f64),
            BinOp::Shl => Value::Number(((self.tn(l) as i32) << ((self.tn(r) as i32) & 31)) as f64),
            BinOp::Shr => Value::Number(((self.tn(l) as i32) >> ((self.tn(r) as i32) & 31)) as f64),
            BinOp::UShr => {
                let a = (self.tn(l) as i32) as u32;
                let b = (self.tn(r) as i32) as u32 & 31;
                Value::Number((a >> b) as f64)
            }
            BinOp::Eq => Value::Bool(self.leq(l, r)),
            BinOp::Neq => Value::Bool(!self.leq(l, r)),
            BinOp::Seq => Value::Bool(self.seq(l, r)),
            BinOp::Sneq => Value::Bool(!self.seq(l, r)),
            BinOp::Lt => Value::Bool(self.tn(l) < self.tn(r)),
            BinOp::Gt => Value::Bool(self.tn(l) > self.tn(r)),
            BinOp::Le => Value::Bool(self.tn(l) <= self.tn(r)),
            BinOp::Ge => Value::Bool(self.tn(l) >= self.tn(r)),
            BinOp::And => {
                if self.truthy(l) {
                    r.clone()
                } else {
                    l.clone()
                }
            }
            BinOp::Or => {
                if self.truthy(l) {
                    l.clone()
                } else {
                    r.clone()
                }
            }
            BinOp::Nullish => {
                if matches!(l, Value::Null | Value::Undefined) {
                    r.clone()
                } else {
                    l.clone()
                }
            }
            BinOp::Comma => r.clone(),
            BinOp::Instanceof => {
                // Internal and host-created errors carry their standard error
                // class in `ErrorData::name` rather than a guest prototype
                // object. Preserve the built-in Error inheritance chain for
                // those values, then use prototype identity for ordinary
                // objects.
                if let (Value::Error(error), Value::Class(class)) = (l, r) {
                    return Ok(Value::Bool(
                        class.name == "Error" || class.name == error.name,
                    ));
                }
                // Date instances use a dedicated VM value, and their built-in
                // constructor identity lives in object metadata instead of an
                // ordinary prototype link.
                if matches!(l, Value::Date(_))
                    && matches!(r, Value::Object { props } if props.meta.borrow().builtin_constructor == Some(crate::value::BuiltinConstructor::Date))
                {
                    return Ok(Value::Bool(true));
                }
                // `l instanceof r`: walk l's prototype chain looking for r's
                // prototype object (compared by shared Rc identity).
                let target_proto = match r {
                    Value::Class(c) => Some(c.prototype.as_ref().clone()),
                    Value::Function(function) => {
                        let prototype = function.prototype_value(r);
                        if !is_js_object(&prototype) {
                            return Err(VmErr::Msg(
                                "TypeError: function has non-object prototype in instanceof check"
                                    .into(),
                            ));
                        }
                        Some(prototype)
                    }
                    _ => None,
                };
                let mut result = false;
                if let Some(tp) = target_proto {
                    let mut cur = l.proto_of();
                    let mut visited = std::collections::HashSet::new();
                    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
                        let Some(p) = cur else {
                            break;
                        };
                        if super::strict_equals(p.as_ref(), &tp) {
                            result = true;
                            break;
                        }
                        let identity = match p.as_ref() {
                            Value::Object { props } => Rc::as_ptr(props) as usize,
                            Value::Class(class) => Rc::as_ptr(&class.statics) as usize,
                            Value::Function(function) => Rc::as_ptr(&function.properties) as usize,
                            Value::Proxy(proxy) => Rc::as_ptr(proxy) as usize,
                            _ => break,
                        };
                        if !visited.insert(identity) {
                            break;
                        }
                        cur = p.proto_of();
                    }
                }
                Value::Bool(result)
            }
            // `in` is a prototype-chain query, not an own-property one, and
            // its left operand is coerced to a property key. A proxy's `has`
            // trap answers it instead when one is defined.
            BinOp::In => {
                let key = match l {
                    Value::String(k) => k.clone(),
                    Value::Number(n) => crate::format::number_string(*n),
                    Value::Symbol(s) => symbol_slot_key(s),
                    other => self.vs(other)?,
                };
                Value::Bool(r.has_prop(&key))
            }
        })
    }

    /// Apply `op` when either operand is a `BigInt`.
    ///
    /// Returns `None` only for operators that have no BigInt-specific
    /// behaviour and can fall through to the general path.
    fn bigint_op(&self, op: BinOp, l: &Value, r: &Value) -> Result<Option<Value>, VmErr> {
        use crate::bigint::BigInt as Big;
        let big = |value: Big| Ok(Some(Value::BigInt(Rc::new(value))));
        let type_error = || {
            Err(VmErr::Msg(
                "TypeError: Cannot mix BigInt and other types, use explicit conversions"
                    .to_string(),
            ))
        };

        // `bigint + string` concatenates, as with any other value.
        if matches!(op, BinOp::Add)
            && (matches!(l, Value::String(_)) || matches!(r, Value::String(_)))
        {
            let joined = format!("{}{}", self.vs(l)?, self.vs(r)?);
            return Ok(Some(Value::checked_string(joined)?));
        }

        // Equality and relational operators compare across the two numeric
        // types; `===` does not, since the types differ.
        match (l.as_bigint(), r.as_bigint()) {
            (Some(a), Some(b)) => {
                let ordering = a.compare(&b);
                Ok(Some(match op {
                    BinOp::Add => return big(a.add(&b).map_err(VmErr::Msg)?),
                    BinOp::Sub => return big(a.sub(&b).map_err(VmErr::Msg)?),
                    BinOp::Mul => return big(a.mul(&b).map_err(VmErr::Msg)?),
                    BinOp::Div => return big(a.div(&b).map_err(VmErr::Msg)?),
                    BinOp::Mod => return big(a.rem(&b).map_err(VmErr::Msg)?),
                    BinOp::Pow => return big(a.pow(&b).map_err(VmErr::Msg)?),
                    BinOp::BitAnd => return big(a.bitand(&b).map_err(VmErr::Msg)?),
                    BinOp::BitOr => return big(a.bitor(&b).map_err(VmErr::Msg)?),
                    BinOp::BitXor => return big(a.bitxor(&b).map_err(VmErr::Msg)?),
                    BinOp::Shl => return big(a.shl(&b).map_err(VmErr::Msg)?),
                    BinOp::Shr | BinOp::UShr => return big(a.shr(&b).map_err(VmErr::Msg)?),
                    BinOp::Lt => Value::Bool(ordering.is_lt()),
                    BinOp::Gt => Value::Bool(ordering.is_gt()),
                    BinOp::Le => Value::Bool(ordering.is_le()),
                    BinOp::Ge => Value::Bool(ordering.is_ge()),
                    BinOp::Eq | BinOp::Seq => Value::Bool(ordering.is_eq()),
                    BinOp::Neq | BinOp::Sneq => Value::Bool(!ordering.is_eq()),
                    _ => return Ok(None),
                }))
            }
            (Some(a), None) => {
                let comparison = numeric_comparand(r).and_then(|n| a.compare_f64(n));
                Ok(Some(match op {
                    BinOp::Lt => Value::Bool(comparison.is_some_and(|o| o.is_lt())),
                    BinOp::Gt => Value::Bool(comparison.is_some_and(|o| o.is_gt())),
                    BinOp::Le => Value::Bool(comparison.is_some_and(|o| o.is_le())),
                    BinOp::Ge => Value::Bool(comparison.is_some_and(|o| o.is_ge())),
                    BinOp::Eq => Value::Bool(comparison.is_some_and(|o| o.is_eq())),
                    BinOp::Neq => Value::Bool(!comparison.is_some_and(|o| o.is_eq())),
                    // Different types, so strict equality is false without
                    // comparing the values.
                    BinOp::Seq => Value::Bool(false),
                    BinOp::Sneq => Value::Bool(true),
                    BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Div
                    | BinOp::Mod
                    | BinOp::Pow
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::BitXor
                    | BinOp::Shl
                    | BinOp::Shr
                    | BinOp::UShr => return type_error(),
                    _ => return Ok(None),
                }))
            }
            (None, Some(b)) => {
                let comparison = numeric_comparand(l).and_then(|n| b.compare_f64(n));
                // `n < bigint` is the mirror of `bigint > n`.
                Ok(Some(match op {
                    BinOp::Lt => Value::Bool(comparison.is_some_and(|o| o.is_gt())),
                    BinOp::Gt => Value::Bool(comparison.is_some_and(|o| o.is_lt())),
                    BinOp::Le => Value::Bool(comparison.is_some_and(|o| o.is_ge())),
                    BinOp::Ge => Value::Bool(comparison.is_some_and(|o| o.is_le())),
                    BinOp::Eq => Value::Bool(comparison.is_some_and(|o| o.is_eq())),
                    BinOp::Neq => Value::Bool(!comparison.is_some_and(|o| o.is_eq())),
                    BinOp::Seq => Value::Bool(false),
                    BinOp::Sneq => Value::Bool(true),
                    BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Div
                    | BinOp::Mod
                    | BinOp::Pow
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::BitXor
                    | BinOp::Shl
                    | BinOp::Shr
                    | BinOp::UShr => return type_error(),
                    _ => return Ok(None),
                }))
            }
            (None, None) => Ok(None),
        }
    }

    pub fn un_op(&self, op: UnOp, v: &Value) -> Result<Value, VmErr> {
        // `-`, `~`, `++` and `--` stay in the BigInt domain; `+` on a BigInt
        // is a TypeError, since it would have to narrow to a Number.
        if let Some(value) = v.as_bigint() {
            let wrap = |result: Result<crate::bigint::BigInt, String>| {
                result
                    .map(|v| Value::BigInt(Rc::new(v)))
                    .map_err(VmErr::Msg)
            };
            match op {
                UnOp::Neg => return Ok(Value::BigInt(Rc::new(value.negate()))),
                UnOp::BitNot => return wrap(value.bitnot()),
                UnOp::Inc => {
                    return wrap(value.add(&crate::bigint::BigInt::from_i64(1)));
                }
                UnOp::Dec => {
                    return wrap(value.sub(&crate::bigint::BigInt::from_i64(1)));
                }
                UnOp::Pos => {
                    return Err(VmErr::Msg(
                        "TypeError: Cannot convert a BigInt to a number".to_string(),
                    ));
                }
                _ => {}
            }
        }
        Ok(match op {
            UnOp::Not => Value::Bool(!self.truthy(v)),
            UnOp::Neg => Value::Number(-self.tn(v)),
            UnOp::Pos => Value::Number(self.tn(v)),
            UnOp::BitNot => Value::Number(!(self.tn(v) as i32) as f64),
            UnOp::Typeof if super::call::callable_slot(v, super::call::CALL_SLOT).is_some() => {
                Value::String("function".to_string())
            }
            UnOp::Typeof if matches!(v, Value::Binding(_)) => {
                return self.un_op(op, &v.deref_binding());
            }
            UnOp::Typeof => Value::String(
                match v {
                    Value::Undefined => "undefined",
                    Value::Null => "object",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Object { .. }
                    | Value::Array(_)
                    | Value::GlobalObject
                    | Value::StringIterator { .. } => "object",
                    Value::Function(_)
                    | Value::NativeFunction { .. }
                    | Value::HostFunction { .. }
                    | Value::Class(_) => "function",
                    Value::Promise { .. } | Value::HostPending { .. } => "object",
                    Value::Generator { .. } => "object",
                    Value::Symbol(_) => "symbol",
                    Value::Error(_) | Value::RegExp(_) => "object",
                    Value::BigInt(_) => "bigint",
                    Value::ArrayBuffer(_)
                    | Value::SharedArrayBuffer(_)
                    | Value::TypedArray(_)
                    | Value::DataView(_)
                    | Value::Date(_) => "object",
                    // A proxy reports the type of what it wraps, so wrapping a
                    // function still reports as one.
                    Value::Proxy(proxy) => {
                        return self.un_op(op, &proxy.target);
                    }
                    // Internal values, resolved before they reach guest code.
                    #[cfg(stackful_coroutines)]
                    Value::AsyncTask(_) => "object",
                    Value::Binding(_) => "undefined",
                }
                .to_string(),
            ),
            UnOp::Void => Value::Undefined,
            UnOp::Delete => Value::Bool(true),
            UnOp::Inc => Value::Number(self.tn(v) + 1.0),
            UnOp::Dec => Value::Number(self.tn(v) - 1.0),
        })
    }

    pub fn keys(&self, o: &Value) -> Vec<String> {
        match o {
            Value::Object { props } => {
                let meta = props.meta.borrow();
                props
                    .borrow()
                    .iter()
                    .filter(|(k, _)| meta.attrs_of(k).enumerable && !is_internal_key(k))
                    .map(|(k, _)| k.clone())
                    .collect()
            }
            // Without an `ownKeys` trap a proxy enumerates its target. The
            // trap needs to call guest code, so it is applied in `Object.keys`
            // and `for…in`, which have `&mut self`.
            Value::Proxy(proxy) => self.keys(&proxy.target),
            Value::Array(i) => (0..i.borrow().len())
                .filter(|index| i.has_index(*index))
                .map(|x| x.to_string())
                .collect(),
            Value::GlobalObject => self.global_keys(),
            _ => vec![],
        }
    }

    pub fn truthy(&self, v: &Value) -> bool {
        v.is_truthy()
    }

    pub fn tn(&self, v: &Value) -> f64 {
        v.to_number()
    }

    pub fn leq(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
            (Value::GlobalObject, Value::GlobalObject) => true,
            (Value::Number(a), Value::String(b)) => {
                if let Ok(parsed) = b.parse::<f64>() {
                    *a == parsed
                } else {
                    false
                }
            }
            (Value::String(a), Value::Number(b)) => {
                if let Ok(parsed) = a.parse::<f64>() {
                    parsed == *b
                } else {
                    false
                }
            }
            (Value::Bool(a), Value::Number(b)) => {
                let num = if *a { 1.0 } else { 0.0 };
                num == *b
            }
            (Value::Number(a), Value::Bool(b)) => {
                let num = if *b { 1.0 } else { 0.0 };
                *a == num
            }
            (Value::Bool(a), Value::String(b)) => {
                let s = if *a { "true" } else { "false" };
                s == b
            }
            (Value::String(a), Value::Bool(b)) => {
                let s = if *b { "true" } else { "false" };
                a == s
            }
            _ => false,
        }
    }

    pub fn seq(&self, a: &Value, b: &Value) -> bool {
        strict_equals(a, b)
    }

    pub fn vs(&self, v: &Value) -> Result<String, VmErr> {
        // Only arrays recurse here (objects print opaquely), so the cycle and
        // depth guards only need to cover the array branch — but both are
        // load-bearing: a cyclic (`a.push(a)`) or million-deep array would
        // otherwise overflow the native stack during stringification.
        let mut visited = std::collections::HashSet::new();
        let mut output = crate::format::BoundedOutput::new(crate::value::MAX_STRING_LEN);
        self.vs_rec(v, &mut visited, 0, &mut output)?;
        Ok(output.finish())
    }

    /// Maximum array nesting rendered by `vs`; deeper levels print as `...`.
    const MAX_PRINT_DEPTH: usize = 128;

    fn vs_rec(
        &self,
        v: &Value,
        visited: &mut std::collections::HashSet<*const ()>,
        depth: usize,
        output: &mut crate::format::BoundedOutput,
    ) -> Result<(), VmErr> {
        match v {
            Value::Binding(cell) => self.vs_rec(&cell.borrow(), visited, depth, output),
            Value::RegExp(re) => {
                output.push_str(&format!("/{}/{}", re.regex.source, re.regex.flags))
            }
            Value::BigInt(value) => output.push_str(&value.to_decimal()),
            // A typed array stringifies as its elements, like an array.
            Value::TypedArray(view) => {
                for index in 0..view.effective_length() {
                    if index > 0 {
                        output.push_char(',')?;
                    }
                    let element =
                        crate::builtins::read_element(view, index).unwrap_or(Value::Undefined);
                    self.vs_rec(&element, visited, depth + 1, &mut *output)?;
                }
                Ok(())
            }
            Value::Proxy(proxy) => self.vs_rec(&proxy.target, visited, depth, output),
            Value::Date(ms) => output.push_str(&crate::builtins::iso_string(ms.get())),
            Value::ArrayBuffer(_) => output.push_str("[object ArrayBuffer]"),
            Value::SharedArrayBuffer(_) => output.push_str("[object SharedArrayBuffer]"),
            Value::DataView(_) => output.push_str("[object DataView]"),
            #[cfg(stackful_coroutines)]
            Value::AsyncTask(_) => output.push_str("[object AsyncTask]"),
            Value::Undefined => output.push_str("undefined"),
            Value::Null => output.push_str("null"),
            Value::Bool(b) => output.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => output.push_str(&crate::format::number_string(*n)),
            Value::String(s) => output.push_str(s),
            Value::Object { .. } => match crate::builtins::describe_collection(v) {
                Some(rendered) => output.push_str(&rendered),
                None => output.push_str("[object Object]"),
            },
            Value::GlobalObject => output.push_str("[object global]"),
            Value::Array(i) => {
                if depth >= Self::MAX_PRINT_DEPTH {
                    return output.push_str("...");
                }
                // Path-based cycle detection (Rc pointer identity): insert
                // on the way down, remove on the way up, so shared-but-acyclic
                // references still print fully.
                let ptr = Rc::as_ptr(i) as *const ();
                if !visited.insert(ptr) {
                    return output.push_str("[Circular]");
                }
                let result = (|| {
                    let items = i.borrow();
                    for (index, item) in items.iter().enumerate() {
                        if index > 0 {
                            output.push_char(',')?;
                        }
                        self.vs_rec(item, visited, depth + 1, output)?;
                    }
                    Ok(())
                })();
                visited.remove(&ptr);
                result
            }
            Value::Function(f) => {
                output.push_str("function ")?;
                output.push_str(f.name.as_deref().unwrap_or(""))
            }
            Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                output.push_str("function ")?;
                output.push_str(name)?;
                output.push_str(" [native]")
            }
            Value::Class(c) => {
                output.push_str("class ")?;
                output.push_str(&c.name)
            }
            Value::Promise { .. } | Value::HostPending { .. } => {
                output.push_str("[object Promise]")
            }
            Value::Generator { .. } => output.push_str("[object Generator]"),
            Value::StringIterator { .. } => output.push_str("[object String Iterator]"),
            Value::Symbol(s) => output.push_str(&s.to_display()),
            Value::Error(e) => output.push_str(&e.message),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn sv(&self, s: &str) -> Value {
        if s == "undefined" {
            Value::Undefined
        } else if s == "null" {
            Value::Null
        } else if s == "true" {
            Value::Bool(true)
        } else if s == "false" {
            Value::Bool(false)
        } else if let Ok(n) = s.parse::<f64>() {
            Value::Number(n)
        } else {
            Value::String(s.to_string())
        }
    }
}

fn is_js_object(value: &Value) -> bool {
    !matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Symbol(_)
            | Value::BigInt(_)
            | Value::HostPending { .. }
            | Value::Binding(_)
    )
}
