//! Statement and expression evaluation: the two big `match` dispatchers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{
    BindKind, Env, Environment, Interpreter, Lookup, ModifyOutcome, block_needs_lexical_scope,
};
use crate::error::{VmErr, vm_err, vm_ret, vm_throw};
use crate::parser::{
    AssignOp, ClassMember, Expr, ExprOrBlock, ForInit, LogicalAssignOp, MemberName, ObjectProp,
    Statement, UnOp, VarKind, arrow_body_references, stmts_reference,
};
use crate::value::{ClassData, FunctionData, ObjectCell, PromiseState, PropAttrs, Value};

/// Convert parser-owned parameter names into interned `Rc<str>` so call-frame
/// binding is a refcount bump, not a heap allocation.
fn intern_params(params: &[String]) -> Rc<Vec<Rc<str>>> {
    Rc::new(params.iter().map(|p| Rc::from(p.as_str())).collect())
}

fn class_accessor_kind(value: &Value) -> Option<&'static str> {
    let name = match value {
        Value::Function(function) => function.name.as_deref(),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            Some(name.as_ref())
        }
        _ => None,
    }?;
    if name.starts_with("get ") {
        Some("get")
    } else if name.starts_with("set ") {
        Some("set")
    } else {
        None
    }
}

fn insert_class_accessor(statics: &mut Vec<(String, Value)>, key: &str, accessor: Value) {
    let companion = format!("__setter:{}__", key);
    let primary = statics.iter().position(|(name, _)| name == key);
    let setter = statics.iter().position(|(name, _)| name == &companion);
    match class_accessor_kind(&accessor) {
        Some("get") => {
            if let Some(index) = primary
                && class_accessor_kind(&statics[index].1) == Some("set")
            {
                let old_setter = statics[index].1.clone();
                if let Some(setter_index) = setter {
                    statics[setter_index].1 = old_setter;
                } else {
                    statics.push((companion, old_setter));
                }
            }
            if let Some(index) = primary {
                statics[index].1 = accessor;
            } else {
                statics.push((key.to_owned(), accessor));
            }
        }
        Some("set") => {
            if primary.is_some_and(|index| class_accessor_kind(&statics[index].1) == Some("get")) {
                if let Some(index) = setter {
                    statics[index].1 = accessor;
                } else {
                    statics.push((companion, accessor));
                }
            } else if let Some(index) = primary {
                statics[index].1 = accessor;
            } else {
                statics.push((key.to_owned(), accessor));
            }
        }
        _ => unreachable!("class accessor must be a getter or setter"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectAccessorKind {
    Getter,
    Setter,
}

/// Add an object-literal property, replacing an earlier value for the same
/// key while preserving a getter/setter pair for accessor properties.
fn insert_object_property(
    props: &mut Vec<Option<(String, Value)>>,
    positions: &mut HashMap<String, Vec<usize>>,
    accessors: &mut HashMap<usize, ObjectAccessorKind>,
    key: String,
    value: Value,
    kind: Option<ObjectAccessorKind>,
) {
    let existing = positions.get(&key).cloned().unwrap_or_default();

    if let Some(kind) = kind {
        if let Some(index) = existing
            .iter()
            .copied()
            .find(|index| accessors.get(index) == Some(&kind))
        {
            props[index] = Some((key, value));
            return;
        }
        if !existing.is_empty() && existing.iter().all(|index| accessors.contains_key(index)) {
            let index = props.len();
            props.push(Some((key.clone(), value)));
            positions.entry(key).or_default().push(index);
            accessors.insert(index, kind);
            return;
        }
    }

    if let Some(index) = existing.first().copied() {
        props[index] = Some((key.clone(), value));
        for duplicate in existing.iter().skip(1) {
            props[*duplicate] = None;
        }
        if let Some(kind) = kind {
            accessors.insert(index, kind);
        } else {
            for index in &existing {
                accessors.remove(index);
            }
        }
        positions.insert(key, vec![index]);
    } else {
        let index = props.len();
        props.push(Some((key.clone(), value)));
        positions.insert(key, vec![index]);
        if let Some(kind) = kind {
            accessors.insert(index, kind);
        }
    }
}

fn push_call_arg(args: &mut Vec<Value>, value: Value) -> Result<(), VmErr> {
    if args.len() >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum argument count exceeded"));
    }
    args.push(value);
    Ok(())
}

/// Whether a labeled control-flow signal targets the loop with `label`.
/// Unlabeled signals (`None`) target the innermost loop and are handled by
/// the callers directly; this only decides labeled ones.
/// Close an iterator that a `for...of` is abandoning before exhaustion.
///
/// Only generators need this today: their bodies may be suspended inside a
/// `try`, and JavaScript runs those `finally` blocks when the loop exits
/// early. Any other iterable is a plain object with no teardown to perform.
#[cfg_attr(not(stackful_coroutines), expect(unused_variables))]
fn close_iterator(iterator: &Value) {
    #[cfg(stackful_coroutines)]
    if let Value::Generator { inner } = iterator {
        // A generator cannot be mid-`next()` here: this runs on the same
        // thread that just returned from it, so the cell is free.
        if let Ok(mut inner) = inner.try_borrow_mut() {
            inner.close();
        }
    }
}

fn label_matches(label: &Option<String>, signal: &Option<String>) -> bool {
    match (label, signal) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Reinterpret an array or object *literal* on the left of `=` as a
/// destructuring pattern.
///
/// The grammar cannot tell `[a, b]` apart from a pattern until the `=` is
/// reached, so the parser produces a literal and this converts it. Anything
/// that is not a valid target yields `None`, which the caller reports.
fn expr_to_pattern(expr: &Expr) -> Option<crate::parser::Pattern> {
    use crate::parser::{Pattern, PatternKey};
    Some(match expr {
        Expr::Identifier(name) => Pattern::Ident(name.clone()),
        Expr::Member {
            object, property, ..
        } => Pattern::Member {
            object: object.clone(),
            property: property.clone(),
        },
        Expr::Array(items) => Pattern::Array(
            items
                .iter()
                .map(|item| match item {
                    Expr::Spread(inner) => {
                        expr_to_pattern(inner).map(|p| Pattern::Rest(Box::new(p)))
                    }
                    // A hole (`[, a] = …`) skips a position.
                    Expr::Undefined => Some(Pattern::Ident("hole".to_string())),
                    other => expr_to_pattern(other),
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        Expr::Object(props) => Pattern::Object(
            props
                .iter()
                .map(|prop| match prop {
                    ObjectProp::Shorthand(name) => Some((PatternKey::Name(name.clone()), None)),
                    ObjectProp::KeyValue(key, value) => {
                        Some((PatternKey::Name(key.clone()), Some(expr_to_pattern(value)?)))
                    }
                    ObjectProp::Spread(inner) => Some((
                        PatternKey::Name("...".to_string()),
                        Some(Pattern::Rest(Box::new(expr_to_pattern(inner)?))),
                    )),
                    ObjectProp::Computed(key, value) => Some((
                        PatternKey::Computed(key.clone()),
                        Some(expr_to_pattern(value)?),
                    )),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        // `[a = 1] = []` supplies a default.
        Expr::Assignment {
            target,
            op: AssignOp::Assign,
            value,
        } => Pattern::Default(
            Box::new(expr_to_pattern(target)?),
            Box::new(value.as_ref().clone()),
        ),
        _ => return None,
    })
}

/// Scope slot holding the superclass prototype inside a class member, so
/// `super.method()` can resolve without the AST carrying a class link.
pub(crate) const SUPER_PROTO: &str = "__super_proto__";

impl Interpreter {
    /// Resolve `super.<key>` — a lookup on the superclass prototype.
    fn super_member(&mut self, key: &Value) -> Result<Value, VmErr> {
        let proto = self
            .global
            .borrow()
            .get(SUPER_PROTO)
            .ok_or_else(|| VmErr::Msg("'super' used outside a derived class".to_string()))?;
        self.get_prop_value(&proto, key)
    }

    /// Build a class value from its parts: prototype methods and accessors,
    /// static members, instance fields desugared into the constructor, and
    /// static blocks run once the class exists.
    ///
    /// Shared by the declaration and the class *expression*, which differ
    /// only in whether the result is bound to a name.
    /// Resolve a class member name: static names as written, computed keys
    /// evaluated once, in definition order, when the class is defined.
    fn member_name(&mut self, name: &MemberName) -> Result<String, VmErr> {
        match name {
            MemberName::Static(name) => Ok(name.clone()),
            MemberName::Computed(expr) => {
                let value = self.eval_expr(expr)?;
                self.property_key(&value)
            }
        }
    }

    fn build_class(
        &mut self,
        name: &str,
        superclass: Option<&Expr>,
        body: &[ClassMember],
    ) -> Result<Value, VmErr> {
        let super_cls = if let Some(sc) = superclass {
            Some(self.eval_expr(sc)?)
        } else {
            None
        };
        // Inheritance: the instance prototype chains to the superclass's
        // prototype so inherited methods resolve.
        let super_proto = match &super_cls {
            Some(Value::Class(c)) => Some(c.prototype.clone()),
            Some(other) => {
                // Native constructors (`Map`, `Set`, `Array`, ...) expose
                // `.prototype` as an ordinary property; inherit from it like
                // a class heritage would.
                match self.get_prop_value(other, &Value::String("prototype".to_string())) {
                    Ok(proto @ (Value::Object { .. } | Value::Array(_))) => Some(Rc::new(proto)),
                    _ => None,
                }
            }
            None => None,
        };

        // Methods, getters and setters close over a scope carrying the
        // superclass prototype, so `super.method()` inside one can find it.
        // The constructor gets `__super_ctor` separately, below.
        let member_closure = match &super_proto {
            Some(proto) => {
                let env = Rc::new(RefCell::new(Environment::child(self.global.clone())));
                env.borrow_mut().set(SUPER_PROTO, proto.as_ref().clone());
                env
            }
            None => self.global.clone(),
        };

        // Gather the constructor, instance fields, and methods.
        let mut ctor_params: Vec<String> = Vec::new();
        let mut ctor_body: Vec<Statement> = Vec::new();
        let mut has_own_constructor = false;
        let mut instance_fields: Vec<(String, Option<Expr>)> = Vec::new();
        let mut proto_props: Vec<(String, Value)> = Vec::new();
        let mut statics: Vec<(String, Value)> =
            vec![("name".to_string(), Value::String(name.to_string()))];
        let mut static_attrs = vec![(
            "name".to_owned(),
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        )];
        let mut static_has_accessors = false;
        let mut static_blocks: Vec<Vec<Statement>> = Vec::new();

        for member in body {
            match member {
                ClassMember::Method {
                    name,
                    is_static: st,
                    params: mp,
                    body: mb,
                    is_async,
                    is_generator,
                } => {
                    let mname = self.member_name(name)?;
                    // Only a written-out `constructor` is the constructor; a
                    // computed key that happens to evaluate to it stays an
                    // ordinary method.
                    let is_ctor_name = matches!(name, MemberName::Static(n) if n == "constructor");
                    let fn_val = Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(mname.as_str().into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: intern_params(mp),
                        body: Rc::new(mb.clone()),
                        closure: Some(member_closure.clone()),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: *is_async,
                        is_generator: *is_generator,
                        uses_arguments: stmts_reference(mb, "arguments"),
                        bound: None,
                    }));
                    if *st {
                        statics.push((mname.clone(), fn_val));
                        static_attrs.push((
                            mname.clone(),
                            PropAttrs {
                                writable: true,
                                enumerable: false,
                                configurable: true,
                            },
                        ));
                    } else if is_ctor_name {
                        has_own_constructor = true;
                        ctor_params = mp.clone();
                        ctor_body = mb.clone();
                    } else {
                        proto_props.push((mname.clone(), fn_val));
                    }
                }
                // Static blocks are collected and run after the class
                // exists, since they observe its statics and `this`.
                ClassMember::StaticBlock { body } => {
                    static_blocks.push(body.clone());
                }
                ClassMember::Field {
                    name,
                    is_static: st,
                    init,
                } => {
                    let fname = self.member_name(name)?;
                    if *st {
                        let init_val = match init {
                            Some(e) => self.eval_expr(e)?,
                            None => Value::Undefined,
                        };
                        statics.push((fname.clone(), init_val));
                        static_attrs.push((fname.clone(), PropAttrs::default()));
                    } else {
                        instance_fields.push((fname.clone(), init.clone()));
                    }
                }
                ClassMember::Getter {
                    name,
                    is_static: st,
                    body: gb,
                } => {
                    let gname = self.member_name(name)?;
                    let getter_fn = Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(format!("get {}", gname).into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: Rc::new(vec![]),
                        body: Rc::new(gb.clone()),
                        closure: Some(member_closure.clone()),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: false,
                        is_generator: false,
                        uses_arguments: stmts_reference(gb, "arguments"),
                        bound: None,
                    }));
                    if *st {
                        insert_class_accessor(&mut statics, &gname, getter_fn);
                        static_attrs.push((
                            gname.clone(),
                            PropAttrs {
                                writable: false,
                                enumerable: false,
                                configurable: true,
                            },
                        ));
                        static_has_accessors = true;
                    } else {
                        proto_props.push((gname.clone(), getter_fn));
                    }
                }
                ClassMember::Setter {
                    name,
                    param,
                    is_static: st,
                    body: sb,
                } => {
                    let sname = self.member_name(name)?;
                    let setter_fn = Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(format!("set {}", sname).into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: Rc::new(vec![Rc::from(param.as_str())]),
                        body: Rc::new(sb.clone()),
                        closure: Some(member_closure.clone()),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: false,
                        is_generator: false,
                        uses_arguments: stmts_reference(sb, "arguments"),
                        bound: None,
                    }));
                    if *st {
                        insert_class_accessor(&mut statics, &sname, setter_fn);
                        static_attrs.push((
                            sname.clone(),
                            PropAttrs {
                                writable: false,
                                enumerable: false,
                                configurable: true,
                            },
                        ));
                        static_has_accessors = true;
                    } else {
                        proto_props.push((sname.clone(), setter_fn));
                    }
                }
            }
        }

        // A derived class with no constructor of its own gets the implicit
        // `constructor(...args) { super(...args); }`. Without it, extending a
        // class whose constructor does the work — `class E extends Error {}` —
        // produced an instance the superclass never initialized.
        if super_cls.is_some() && !has_own_constructor {
            ctor_params = vec!["...args".to_string()];
            ctor_body = vec![Statement::Expr(Expr::Call {
                callee: Box::new(Expr::Super),
                args: vec![Expr::Spread(Box::new(Expr::Identifier("args".to_string())))],
            })];
        }

        // Desugar instance fields into `this.<field> = <init>;` statements
        // prepended to the constructor body.
        let mut full_ctor_body = Vec::new();
        for (fname, init) in instance_fields {
            let value = init.unwrap_or(Expr::Undefined);
            full_ctor_body.push(Statement::Expr(Expr::Assignment {
                target: Box::new(Expr::Member {
                    object: Box::new(Expr::This),
                    property: Box::new(Expr::String(fname.clone())),
                    computed: false,
                }),
                op: AssignOp::Assign,
                value: Box::new(value),
            }));
        }
        full_ctor_body.extend(ctor_body);

        // For a derived class, expose the superclass constructor to the
        // constructor body as `__super_ctor` so `super(...)` can call it. A
        // native heritage is its own `super(...)` target.
        let super_ctor_value = match &super_cls {
            Some(Value::Class(sc)) => Some(sc.constructor.as_ref().clone()),
            Some(other) if super::call::is_callable_value(other) => Some(other.clone()),
            _ => None,
        };
        let ctor_closure = match super_ctor_value {
            Some(target) => {
                let env = Rc::new(RefCell::new(Environment::child(self.global.clone())));
                env.borrow_mut().set("__super_ctor", target);
                env
            }
            None => self.global.clone(),
        };

        let constructor_length = ctor_params
            .iter()
            .take_while(|parameter| !parameter.starts_with("..."))
            .count();
        let constructor = Value::Function(Box::new(FunctionData {
            identity: Rc::new(0),
            name: Some(Rc::from(name)),
            properties: FunctionData::properties_with_default_prototype(&self.persistent_global),
            standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
            params: Rc::new(
                ctor_params
                    .into_iter()
                    .map(|p| Rc::from(p.as_str()))
                    .collect(),
            ),
            uses_arguments: stmts_reference(&full_ctor_body, "arguments"),
            body: Rc::new(full_ctor_body),
            closure: Some(ctor_closure),
            is_arrow: false,
            is_constructor: false,
            is_async: false,
            is_generator: false,
            bound: None,
        }));

        let prototype = Value::object_with_proto(proto_props, super_proto);
        prototype.set_prop("constructor".to_string(), constructor.clone())?;

        statics.push((
            "length".to_owned(),
            Value::Number(constructor_length as f64),
        ));
        static_attrs.push((
            "length".to_owned(),
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        ));
        statics.push(("prototype".to_owned(), prototype.clone()));
        static_attrs.push((
            "prototype".to_owned(),
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        ));
        let static_properties = Rc::new(ObjectCell::new_with_default_proto(statics));
        if let Some(superclass) = &super_cls {
            static_properties.set_proto(Some(Rc::new(superclass.clone())));
        } else if let Some(function_prototype) =
            FunctionData::default_function_prototype(&self.persistent_global)
        {
            static_properties.set_proto(Some(Rc::new(function_prototype)));
        }
        {
            let mut meta = static_properties.meta.borrow_mut();
            for (key, attrs) in static_attrs {
                meta.set_attrs(&key, attrs);
            }
            meta.has_accessors = static_has_accessors;
        }

        let class_val = Value::Class(Box::new(ClassData {
            name: name.to_string(),
            constructor: Box::new(constructor),
            prototype: Rc::new(prototype),
            statics: static_properties,
        }));
        if let Value::Class(class) = &class_val {
            class
                .prototype
                .as_ref()
                .set_prop("constructor".to_owned(), class_val.clone())?;
            if let Value::Object { props } = class.prototype.as_ref() {
                props.meta.borrow_mut().set_attrs(
                    "constructor",
                    PropAttrs {
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }

        // The class binds its own name inside static blocks and
        // method bodies, so `static { A.y = … }` can reach it.
        for block in static_blocks {
            let scope = Rc::new(RefCell::new(Environment::child(self.global.clone())));
            scope.borrow_mut().set("this", class_val.clone());
            scope.borrow_mut().set(name, class_val.clone());
            let saved = std::mem::replace(&mut self.global, scope);
            let result = self.run_program_body(&block);
            self.global = saved;
            result?;
        }
        Ok(class_val)
    }

    pub(super) fn eval_stmt(&mut self, s: &Statement) -> Result<Value, VmErr> {
        match s {
            Statement::Expr(e) => self.eval_expr(e),
            Statement::VarDecl {
                name,
                init,
                destructuring,
                kind,
            } => {
                let v = match init {
                    Some(e) => self.eval_expr(e)?,
                    None => Value::Undefined,
                };
                match kind {
                    // `var` was already hoisted to the enclosing function or
                    // program scope; this statement only assigns to it.
                    VarKind::Var => {
                        if let Some(pat) = destructuring {
                            self.destructure(pat, &v)?;
                        } else if init.is_some() {
                            self.assign_or_set_binding(name, v.clone())?;
                        } else {
                            // A bare `var a;` re-declaration must not erase an
                            // existing value: `var a = 1; var a; a` is 1.
                            // Hoisting already created the binding.
                            if self.global.borrow().get(name).is_none() {
                                self.assign_or_set_binding(name, Value::Undefined)?;
                            }
                        }
                    }
                    // `let`/`const` bind in *this* block, leaving the dead
                    // zone. Declaring here as well as in `hoist_lexical` keeps
                    // the statement correct on the paths that do not hoist.
                    VarKind::Let | VarKind::Const => {
                        let bind_kind = if matches!(kind, VarKind::Const) {
                            BindKind::Const
                        } else {
                            BindKind::Let
                        };
                        match destructuring {
                            Some(pat) => {
                                // Declare each name first so `destructure`'s
                                // writes land on bindings that already carry
                                // the right kind -- otherwise a destructured
                                // `const` would be reassignable.
                                for bound in crate::parser::pattern_names(pat) {
                                    self.declare_binding(
                                        &bound,
                                        Value::Undefined,
                                        bind_kind,
                                        false,
                                    )?;
                                }
                                self.destructure(pat, &v)?;
                            }
                            None => {
                                self.declare_binding(name, v.clone(), bind_kind, true)?;
                            }
                        }
                    }
                }
                Ok(v)
            }
            Statement::FnDecl {
                name,
                params,
                body,
                is_async,
                is_generator,
            } => {
                self.set_binding(
                    name,
                    Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(name.as_str().into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: intern_params(params),
                        body: Rc::new(body.clone()),
                        closure: Some(self.global.clone()),
                        is_arrow: false,
                        is_constructor: !*is_async && !*is_generator,
                        is_async: *is_async,
                        is_generator: *is_generator,
                        uses_arguments: stmts_reference(body, "arguments"),
                        bound: None,
                    })),
                )?;
                Ok(Value::Undefined)
            }
            Statement::ClassDecl {
                name,
                superclass,
                body,
            } => {
                let class_val = self.build_class(name, superclass.as_deref(), body)?;
                self.set_binding(name, class_val)?;
                Ok(Value::Undefined)
            }
            Statement::Return(e) => {
                let v = match e {
                    Some(ex) => self.eval_expr(ex)?,
                    None => Value::Undefined,
                };
                vm_ret(v)
            }
            Statement::If { test, then, else_ } => {
                let t = self.eval_expr(test)?;
                if self.truthy(&t) {
                    self.run_block(then)
                } else if let Some(a) = else_ {
                    self.run_block(a)
                } else {
                    Ok(Value::Undefined)
                }
            }
            Statement::While { test, body } => {
                let body_needs_scope = block_needs_lexical_scope(body);
                let label = self.active_label.take();
                let mut r = Value::Undefined;
                loop {
                    self.consume_loop()?;
                    let t = self.eval_expr(test)?;
                    if !self.truthy(&t) {
                        break;
                    }
                    match self.run_block_with_lexical_scope(body, body_needs_scope) {
                        Err(VmErr::Break(None)) => break,
                        Err(VmErr::Break(l)) if label_matches(&label, &l) => break,
                        Err(VmErr::Continue(None)) => continue,
                        Err(VmErr::Continue(l)) if label_matches(&label, &l) => continue,
                        other => r = other?,
                    }
                }
                Ok(r)
            }
            Statement::DoWhile { test, body } => {
                let body_needs_scope = block_needs_lexical_scope(body);
                let label = self.active_label.take();
                let mut r = Value::Undefined;
                loop {
                    self.consume_loop()?;
                    match self.run_block_with_lexical_scope(body, body_needs_scope) {
                        Err(VmErr::Break(None)) => break,
                        Err(VmErr::Break(l)) if label_matches(&label, &l) => break,
                        Err(VmErr::Continue(None)) => {}
                        Err(VmErr::Continue(l)) if label_matches(&label, &l) => {}
                        other => r = other?,
                    }
                    let t = self.eval_expr(test)?;
                    if !self.truthy(&t) {
                        break;
                    }
                }
                Ok(r)
            }
            Statement::For {
                init,
                test,
                update,
                body,
            } => {
                // The loop head gets its own scope, so `for (let i = ...)`
                // does not leak `i` and does not collide with an outer `i`.
                let outer = self.push_scope();
                let result =
                    self.run_for(init.as_deref(), test.as_deref(), update.as_deref(), body);
                self.pop_scope(outer);
                result
            }
            Statement::ForIn { name, obj, body } => {
                let o = self.eval_expr(obj)?;
                let ks = self.keys_with_proxy_trap(&o)?;
                let body_needs_scope = block_needs_lexical_scope(body);
                let mut r = Value::Undefined;
                let label = self.active_label.take();
                for k in ks {
                    self.consume_loop()?;
                    self.set_binding(name, Value::String(k))?;
                    match self.run_block_with_lexical_scope(body, body_needs_scope) {
                        Err(VmErr::Break(None)) => break,
                        Err(VmErr::Break(l)) if label_matches(&label, &l) => break,
                        Err(VmErr::Continue(None)) => continue,
                        Err(VmErr::Continue(l)) if label_matches(&label, &l) => continue,
                        other => r = other?,
                    }
                }
                Ok(r)
            }
            Statement::ForOf {
                name,
                pattern,
                iter,
                body,
                is_await,
            } => {
                let source = self.eval_expr(iter)?;
                let body_needs_scope = block_needs_lexical_scope(body);
                let iterator = if *is_await {
                    self.async_iterator_for(&source)?
                } else {
                    self.iterator_for(&source)?
                };
                let next_fn = self.prop(&iterator, &Value::String("next".to_string()))?;
                if matches!(next_fn, Value::Undefined) {
                    return vm_err("iterator has no next() method");
                }
                let mut r = Value::Undefined;
                let label = self.active_label.take();
                // Leaving before the iterator reports `done` must close it, so
                // a suspended generator runs its `finally` blocks. Tracked here
                // and acted on at every exit, error paths included.
                let mut exhausted = false;
                loop {
                    // Account for the iterator's next call as well as the
                    // body iteration. This keeps custom/infinite iterators
                    // budgeted without eagerly collecting their output.
                    self.consume_loop()?;
                    let mut result = self.call_this(&next_fn, iterator.clone(), vec![])?;
                    // `for await` awaits the step object itself, which is what
                    // lets an async iterator return a promise of `{value,
                    // done}` rather than the object directly.
                    if *is_await {
                        result = self.perform_await(result)?;
                    }
                    let done = result
                        .get_prop("done")
                        .map(|v| v.is_truthy())
                        .unwrap_or(true);
                    if done {
                        exhausted = true;
                        break;
                    }
                    let mut value = result.get_prop("value").unwrap_or(Value::Undefined);
                    // A sync iterator of promises is also valid input to
                    // `for await`, so each value is awaited too.
                    if *is_await {
                        value = self.perform_await(value)?;
                    }
                    match pattern {
                        Some(pattern) => {
                            for bound in crate::parser::pattern_names(pattern) {
                                self.declare_binding(
                                    &bound,
                                    Value::Undefined,
                                    BindKind::Let,
                                    false,
                                )?;
                            }
                            if let Err(error) = self.destructure(pattern, &value) {
                                // An abandon teardown runs no handlers — and
                                // closing is a handler. Anything else closes.
                                if !error.is_abandon() {
                                    close_iterator(&iterator);
                                }
                                return Err(error);
                            }
                        }
                        None => self.set_binding(name, value)?,
                    }
                    match self.run_block_with_lexical_scope(body, body_needs_scope) {
                        Err(VmErr::Break(None)) => break,
                        Err(VmErr::Break(l)) if label_matches(&label, &l) => break,
                        Err(VmErr::Continue(None)) => continue,
                        Err(VmErr::Continue(l)) if label_matches(&label, &l) => continue,
                        // `return`, `throw`, or a break/continue aimed at an
                        // outer label also leaves the loop, and also closes.
                        // An abandon teardown is the exception: it runs no
                        // handlers, so it must not close either.
                        Err(error) => {
                            if !error.is_abandon() {
                                close_iterator(&iterator);
                            }
                            return Err(error);
                        }
                        Ok(value) => r = value,
                    }
                }
                if !exhausted {
                    close_iterator(&iterator);
                }
                Ok(r)
            }
            Statement::Block(s) => self.run_block(s),
            // A declarator group shares the enclosing scope: no new frame.
            Statement::Declarations(s) => self.run(s),
            Statement::Labeled { label, body } => {
                // Make the label available to a directly-wrapped loop, which
                // takes it on entry.
                let prev = self.active_label.take();
                self.active_label = Some(label.clone());
                let r = self.eval_stmt(body);
                self.active_label = prev;
                match r {
                    // Consume a labeled break that escaped a non-loop body.
                    Err(VmErr::Break(Some(l))) if l == *label => Ok(Value::Undefined),
                    other => other,
                }
            }
            Statement::Break => Err(VmErr::Break(None)),
            Statement::Continue => Err(VmErr::Continue(None)),
            Statement::LabeledBreak(label) => Err(VmErr::Break(Some(label.clone()))),
            Statement::LabeledContinue(label) => Err(VmErr::Continue(Some(label.clone()))),
            Statement::Throw(e) => {
                let v = self.eval_expr(e)?;
                vm_throw(v)
            }
            Statement::Try {
                body,
                catch,
                finally,
            } => {
                // Run the body, routing thrown and runtime errors into catch.
                // An abandon teardown bypasses both `catch` and `finally`:
                // it runs no guest code on the way out.
                let body_result = self.run_block(body);
                if let Err(error) = &body_result
                    && error.is_abandon()
                {
                    return body_result;
                }

                let after_catch = match body_result {
                    Err(VmErr::Throw(val)) => self.run_catch(catch, val),
                    // Control-flow signals are not catchable.
                    Err(e @ (VmErr::Break(_) | VmErr::Continue(_))) => Err(e),
                    // Runtime errors (e.g. undeclared identifier, limit guards)
                    // are catchable as real error objects with `name`/`message`.
                    Err(VmErr::Msg(m)) => {
                        let frames = self.get_stack().to_vec();
                        let value = crate::error::error_value_with_stack(&m, &frames);
                        self.run_catch(catch, value)
                    }
                    // A located runtime error already carries the stack from
                    // where it was raised, which is deeper than here.
                    Err(VmErr::RuntimeError(re)) => {
                        let value = crate::error::error_value_with_stack(&re.message, &re.stack);
                        self.run_catch(catch, value)
                    }
                    other => other,
                };

                // finally always runs last; its own error/return takes precedence.
                if let Some(f) = finally {
                    self.run_block(f)?;
                }
                after_catch
            }
            Statement::Switch { disc, cases } => {
                let d = self.eval_expr(disc)?;
                // Every case shares one block scope: fall-through means a
                // `let` declared in one case is visible in the next.
                let outer = self.push_scope();
                let result = self.run_switch_cases(&d, cases);
                self.pop_scope(outer);
                result
            }
            Statement::ExportDefault(e) => {
                let v = self.eval_expr(e)?;
                let mn = self.cur_mod.clone().unwrap_or_default();
                let _ = mn;
                self.current_module().default = Some(v);
                Ok(Value::Undefined)
            }
            Statement::ExportNamed { specifiers, source } => {
                match source {
                    // `export { a, b as c } from 'm'`: forward the *other*
                    // module's live bindings without binding anything locally.
                    Some(source) => {
                        let entries = self.resolve_reexports(source, specifiers)?;
                        let mut record = self.current_module();
                        for (exported, value) in entries {
                            if exported == "default" {
                                record.default = Some(value);
                            } else {
                                record.exports.insert(exported, value);
                            }
                        }
                    }
                    // `export { a, b as c }`: publish this module's own
                    // bindings as live cells, so a later write is observed by
                    // every importer.
                    None => {
                        // A name an importer already bound during a cycle has
                        // a cell waiting; adopt it so the value lands where
                        // that importer is looking, instead of in a new one.
                        let promised: Vec<(String, Option<Value>)> = {
                            let record = self.current_module();
                            specifiers
                                .iter()
                                .map(|(_, exported)| {
                                    (exported.clone(), record.exports.get(exported).cloned())
                                })
                                .collect()
                        };
                        let mut cells = Vec::with_capacity(specifiers.len());
                        {
                            let mut scope = self.global.borrow_mut();
                            for ((local, exported), (_, existing)) in
                                specifiers.iter().zip(promised)
                            {
                                if let Some(Value::Binding(cell)) = &existing {
                                    scope.adopt_cell(local, cell.clone());
                                    cells.push((exported.clone(), Value::Binding(cell.clone())));
                                    continue;
                                }
                                if let Some(cell) = scope.export_cell(local) {
                                    cells.push((exported.clone(), Value::Binding(cell.clone())));
                                }
                            }
                        }
                        let mut record = self.current_module();
                        for (exported, value) in cells {
                            if exported == "default" {
                                record.default = Some(value);
                            } else {
                                record.exports.insert(exported, value);
                            }
                        }
                    }
                }
                Ok(Value::Undefined)
            }
            Statement::ExportAll { source, alias } => {
                let resolved = self
                    .resolve_module_name(source)
                    .ok_or_else(|| VmErr::Msg(format!("Module not found: {}", source)))?;
                self.ensure_module(&resolved)?;
                let other = self
                    .module(&resolved)
                    .ok_or_else(|| VmErr::Msg(format!("Module not found: {}", source)))?;
                match alias {
                    // `export * as ns from 'm'`: one export holding the
                    // namespace object.
                    Some(alias) => {
                        let namespace = Self::namespace_object(&other)?;
                        self.current_module()
                            .exports
                            .insert(alias.clone(), namespace);
                    }
                    // `export * from 'm'`: every *named* export of `m`, which
                    // deliberately excludes its default.
                    None => {
                        let mut record = self.current_module();
                        for (name, value) in other.exports {
                            record.exports.insert(name, value);
                        }
                    }
                }
                Ok(Value::Undefined)
            }
            Statement::Import {
                module,
                default,
                named,
                namespace,
            } => {
                let resolved_module = self.resolve_module_name(module);
                if let Some(name) = resolved_module.as_ref() {
                    let name = name.clone();
                    self.ensure_module(&name)?;
                }
                if let Some(md) = resolved_module.as_ref().and_then(|name| self.module(name)) {
                    if let Some(d) = default {
                        let v = md.default.clone().unwrap_or(Value::Undefined);
                        self.bind_import(d, v)?;
                    }
                    for (imported, local) in named {
                        // `import { default as x }` names the default export.
                        let v = if imported == "default" {
                            md.default.clone().unwrap_or(Value::Undefined)
                        } else {
                            match md.exports.get(imported).cloned() {
                                Some(entry) => entry,
                                // Not exported *yet*: in a cycle the exporting
                                // module is still running, so bind the cell it
                                // will fill in when its `export` runs.
                                None => {
                                    let target = resolved_module.clone().unwrap_or_default();
                                    self.pending_export(&target, imported)
                                        .unwrap_or(Value::Undefined)
                                }
                            }
                        };
                        self.bind_import(local, v)?;
                    }
                    if let Some(ns) = namespace {
                        let namespace_object = Self::namespace_object(&md)?;
                        self.set_binding(ns, namespace_object)?;
                    }
                    Ok(Value::Undefined)
                } else {
                    if module.starts_with('.') && self.cur_mod.is_none() {
                        vm_err(format!(
                            "Relative import requires a module context: {}",
                            module
                        ))
                    } else {
                        vm_err(format!("Module not found: {}", module))
                    }
                }
            }
            Statement::Empty => Ok(Value::Undefined),
        }
    }

    /// Collect every value an iterable produces, for spread and rest.
    ///
    /// Bounded by the loop budget and the array-length cap, so an infinite
    /// generator raises a catchable `RangeError` instead of hanging.
    pub(crate) fn drain_iterable(&mut self, source: &Value) -> Result<Vec<Value>, VmErr> {
        let iterator = self.iterator_for(source)?;
        let next_fn = self.prop(&iterator, &Value::String("next".to_string()))?;
        if matches!(next_fn, Value::Undefined) {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        loop {
            self.consume_loop()?;
            let step = self.call_this(&next_fn, iterator.clone(), vec![])?;
            let done = step.get_prop("done").map(|v| v.is_truthy()).unwrap_or(true);
            if done {
                return Ok(out);
            }
            out.push(step.get_prop("value").unwrap_or(Value::Undefined));
            if out.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
        }
    }

    /// Obtain an iterator for `for await (… of source)`.
    ///
    /// `Symbol.asyncIterator` wins when the source has one; otherwise the
    /// synchronous iterator is used and each of its values is awaited, which
    /// is how `for await` consumes an array of promises.
    fn async_iterator_for(&mut self, source: &Value) -> Result<Value, VmErr> {
        let key = crate::builtins::well_known("asyncIterator")
            .unwrap_or(Value::String("Symbol.asyncIterator".to_string()));
        let async_iter_fn = self.prop(source, &key)?;
        if !matches!(async_iter_fn, Value::Undefined) {
            return self.call_this(&async_iter_fn, source.clone(), vec![]);
        }
        self.iterator_for(source)
    }

    /// Obtain an iterator for `source`, following the `Symbol.iterator`
    /// protocol. Shared by `for...of` and `yield*`.
    fn iterator_for(&mut self, source: &Value) -> Result<Value, VmErr> {
        if matches!(source, Value::String(_)) {
            let iter_fn = self.prop(source, &Value::String("__symbol_iterator__".to_string()))?;
            return self.call_this(&iter_fn, source.clone(), vec![]);
        }
        match source {
            // A generator is its own iterator.
            Value::Generator { .. } => Ok(source.clone()),
            Value::Array(_) => {
                let iter_fn =
                    self.prop(source, &Value::String("__symbol_iterator__".to_string()))?;
                self.call_this(&iter_fn, source.clone(), vec![])
            }
            Value::Object { .. } | Value::TypedArray(_) => {
                let iter_fn =
                    self.prop(source, &Value::String("__symbol_iterator__".to_string()))?;
                if matches!(iter_fn, Value::Undefined) {
                    return vm_err("object is not iterable (no Symbol.iterator)");
                }
                self.call_this(&iter_fn, source.clone(), vec![])
            }
            other => {
                let rendered = self.vs(other).unwrap_or_else(|_| "value".to_string());
                vm_err(format!("TypeError: {} is not iterable", rendered))
            }
        }
    }

    /// The body of a C-style `for`, running inside the loop scope the caller
    /// pushed.
    ///
    /// `for (let i = ...)` gives **each iteration its own binding**, which is
    /// what makes a closure created in the body capture that iteration's
    /// value:
    ///
    /// ```js
    /// const fns = [];
    /// for (let i = 0; i < 3; i++) fns.push(() => i);
    /// fns.map(f => f());   // [0, 1, 2], not [3, 3, 3]
    /// ```
    ///
    /// The copy happens *after* the body and *before* the update, so the
    /// update advances the next iteration's binding rather than the one the
    /// body just captured. `var` keeps the single function-scoped binding, so
    /// the same loop written with `var` still yields `[3, 3, 3]`.
    fn run_for(
        &mut self,
        init: Option<&ForInit>,
        test: Option<&Expr>,
        update: Option<&Expr>,
        body: &[Statement],
    ) -> Result<Value, VmErr> {
        let loop_scope = self.global.clone();
        let mut per_iteration: Vec<String> = Vec::new();

        if let Some(init) = init {
            match init {
                ForInit::Var { kind, decls } => {
                    for (name, init) in decls {
                        let v = match init {
                            Some(e) => self.eval_expr(e)?,
                            None => Value::Undefined,
                        };
                        match kind {
                            // Hoisted to the function scope already.
                            VarKind::Var => self.assign_or_set_binding(name, v)?,
                            VarKind::Let => {
                                self.declare_binding(name, v, BindKind::Let, true)?;
                                per_iteration.push(name.clone());
                            }
                            VarKind::Const => {
                                self.declare_binding(name, v, BindKind::Const, true)?
                            }
                        }
                    }
                }
                ForInit::Pattern {
                    kind,
                    pattern,
                    init,
                    trailing,
                } => {
                    // Mirrors `VarDecl`: every name exists (with the right
                    // kind) before the pattern binds, then the trailing
                    // declarators run in order.
                    let v = self.eval_expr(init)?;
                    let mut names = crate::parser::pattern_names(pattern);
                    names.extend(trailing.iter().map(|(name, _)| name.clone()));
                    match kind {
                        // Hoisted to the function scope already.
                        VarKind::Var => {
                            // Destructure into a detached scope, then publish
                            // each name outward: `assign_or_set_binding`
                            // walks past the loop scope to the hoisted
                            // function-scope bindings, like a plain
                            // `for (var i …)` — writing in place would strand
                            // the values on the loop scope, which is popped.
                            let loop_scope = self.global.clone();
                            self.global =
                                Rc::new(RefCell::new(Environment::child(loop_scope.clone())));
                            let destructured = self.destructure(pattern, &v);
                            let temp = self.global.clone();
                            self.global = loop_scope;
                            destructured?;
                            for name in crate::parser::pattern_names(pattern) {
                                // Each lookup ends before its write: the
                                // borrow guard must not outlive the statement.
                                let value = temp.borrow().get(&name);
                                if let Some(value) = value {
                                    self.assign_or_set_binding(&name, value)?;
                                }
                            }
                            for (name, init) in trailing {
                                let value = match init {
                                    Some(e) => self.eval_expr(e)?,
                                    None => Value::Undefined,
                                };
                                self.assign_or_set_binding(name, value)?;
                            }
                        }
                        VarKind::Let | VarKind::Const => {
                            let bind_kind = if matches!(kind, VarKind::Const) {
                                BindKind::Const
                            } else {
                                BindKind::Let
                            };
                            for name in &names {
                                self.declare_binding(name, Value::Undefined, bind_kind, true)?;
                                if matches!(kind, VarKind::Let) {
                                    per_iteration.push(name.clone());
                                }
                            }
                            self.destructure(pattern, &v)?;
                            for (name, init) in trailing {
                                if let Some(e) = init {
                                    let value = self.eval_expr(e)?;
                                    self.assign_or_set_binding(name, value)?;
                                }
                            }
                        }
                    }
                }
                ForInit::Expr(e) => {
                    self.eval_expr(e)?;
                }
            }
        }

        let needs_per_iteration_environment = !per_iteration.is_empty()
            && crate::parser::for_loop_captures_bindings(init, test, update, body, &per_iteration);
        let body_needs_scope = block_needs_lexical_scope(body);

        if needs_per_iteration_environment {
            self.global = self.copy_iteration_scope(&loop_scope, &per_iteration);
        }

        let mut r = Value::Undefined;
        let label = self.active_label.take();
        loop {
            self.consume_loop()?;
            if let Some(t) = test {
                let tv = self.eval_expr(t)?;
                if !self.truthy(&tv) {
                    break;
                }
            }
            match self.run_block_with_lexical_scope(body, body_needs_scope) {
                Err(VmErr::Break(None)) => break,
                Err(VmErr::Break(l)) if label_matches(&label, &l) => break,
                Err(VmErr::Continue(None)) => {}
                Err(VmErr::Continue(l)) if label_matches(&label, &l) => {}
                other => r = other?,
            }
            if needs_per_iteration_environment {
                let current = self.global.clone();
                self.global = self.copy_iteration_scope(&current, &per_iteration);
            }
            if let Some(u) = update {
                self.eval_expr(u)?;
            }
        }
        Ok(r)
    }

    /// Build the next iteration's scope: a sibling of `from` carrying a fresh
    /// copy of each per-iteration binding's current value.
    fn copy_iteration_scope(&self, from: &Env, names: &[String]) -> Env {
        let parent = from
            .borrow()
            .parent_env()
            .unwrap_or_else(|| self.persistent_global.clone());
        let scope = Rc::new(RefCell::new(Environment::child(parent)));
        {
            let source = from.borrow();
            let mut target = scope.borrow_mut();
            for name in names {
                let value = source.get(name).unwrap_or(Value::Undefined);
                target.declare(name, value, BindKind::Let, true);
            }
        }
        scope
    }

    fn eval_object_literal(&mut self, props: &[ObjectProp]) -> Result<Value, VmErr> {
        let mut object = Vec::new();
        let mut positions = HashMap::new();
        let mut accessors = HashMap::new();
        let mut symbol_keys = Vec::new();
        for prop in props {
            match prop {
                ObjectProp::Shorthand(name) => {
                    let value = self.global.borrow().get(name).unwrap_or(Value::Undefined);
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        name.clone(),
                        value,
                        None,
                    );
                }
                ObjectProp::KeyValue(key, expression) => {
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        key.clone(),
                        self.eval_expr(expression)?,
                        None,
                    );
                }
                ObjectProp::Computed(key_expression, value_expression) => {
                    let key_value = self.eval_expr(key_expression)?;
                    let symbol = match &key_value {
                        Value::Symbol(symbol) => Some(symbol.clone()),
                        _ => None,
                    };
                    let key = match &key_value {
                        Value::String(value) => value.clone(),
                        Value::Number(value) => value.to_string(),
                        Value::Symbol(value) => super::symbol_slot_key(value),
                        _ => continue,
                    };
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        key.clone(),
                        self.eval_expr(value_expression)?,
                        None,
                    );
                    if let Some(symbol) = symbol {
                        symbol_keys.push((key, symbol));
                    }
                }
                ObjectProp::Method {
                    name,
                    params,
                    body,
                    is_async,
                    is_generator,
                } => {
                    let function = Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(name.as_str().into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: intern_params(params),
                        body: Rc::new(body.clone()),
                        closure: Some(self.global.clone()),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: *is_async,
                        is_generator: *is_generator,
                        uses_arguments: stmts_reference(body, "arguments"),
                        bound: None,
                    }));
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        name.clone(),
                        function,
                        None,
                    );
                }
                ObjectProp::Getter { name, body } => {
                    let function = Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(format!("get {name}").into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: Rc::new(vec![]),
                        body: Rc::new(body.clone()),
                        closure: Some(self.global.clone()),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: false,
                        is_generator: false,
                        uses_arguments: stmts_reference(body, "arguments"),
                        bound: None,
                    }));
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        name.clone(),
                        function,
                        Some(ObjectAccessorKind::Getter),
                    );
                }
                ObjectProp::Setter { name, param, body } => {
                    let function = Value::Function(Box::new(FunctionData {
                        identity: Rc::new(0),
                        name: Some(format!("set {name}").into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: Rc::new(vec![Rc::from(param.as_str())]),
                        body: Rc::new(body.clone()),
                        closure: Some(self.global.clone()),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: false,
                        is_generator: false,
                        uses_arguments: stmts_reference(body, "arguments"),
                        bound: None,
                    }));
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        name.clone(),
                        function,
                        Some(ObjectAccessorKind::Setter),
                    );
                }
                ObjectProp::Spread(expression) => {
                    let value = self.eval_expr(expression)?;
                    if matches!(&value, Value::Object { .. } | Value::Proxy(_)) {
                        for key in self.keys_with_proxy_trap(&value)? {
                            let property_value = self.member(&value, &key)?;
                            insert_object_property(
                                &mut object,
                                &mut positions,
                                &mut accessors,
                                key.clone(),
                                property_value,
                                None,
                            );
                            if positions.len() > crate::value::MAX_OBJECT_PROPS {
                                return Err(crate::value::limit_err(
                                    "Maximum object property count exceeded",
                                ));
                            }
                        }
                    }
                }
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

    /// Run a `switch`'s cases inside the scope the caller already pushed.
    ///
    /// All cases share that one block scope, because fall-through means a
    /// `let` declared by one case is in scope for the next. Lexical
    /// declarations from *every* case are hoisted before any case runs, so a
    /// case that falls into a later declaration sees a dead zone rather than
    /// an outer binding.
    fn run_switch_cases(
        &mut self,
        disc: &Value,
        cases: &[crate::parser::SwitchCase],
    ) -> Result<Value, VmErr> {
        for case in cases {
            self.hoist_lexical_public(&case.body)?;
        }

        let mut r = Value::Undefined;
        let mut matched = false;
        let mut found_label = None;
        for c in cases {
            if let Some(ref t) = c.test {
                let tv = self.eval_expr(t)?;
                if self.seq(disc, &tv) {
                    matched = true;
                }
            } else {
                matched = true;
            }
            if matched {
                match self.run(&c.body) {
                    Err(VmErr::Break(l)) => match l {
                        None => break,
                        Some(label) => {
                            found_label = Some(label);
                            break;
                        }
                    },
                    Err(e) => return Err(e),
                    Ok(v) => {
                        r = v;
                    }
                }
            }
        }
        if let Some(label) = found_label {
            return Err(VmErr::Break(Some(label)));
        }
        Ok(r)
    }

    pub(crate) fn eval_expr(&mut self, e: &Expr) -> Result<Value, VmErr> {
        match e {
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::String(s) => {
                if s.len() > crate::value::MAX_STRING_LEN {
                    return Err(crate::value::limit_err("Maximum string length exceeded"));
                }
                Ok(Value::String(s.clone()))
            }
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            // Each evaluation compiles a fresh object, so two literals with
            // the same source have separate `lastIndex` state — as in a real
            // engine, where a literal creates a new `RegExp` each time.
            Expr::Regex(pattern, flags) => crate::builtins::compile_regex(pattern, flags),
            Expr::BigIntLiteral(digits) => match crate::bigint::BigInt::parse(digits) {
                Ok(value) => Ok(Value::BigInt(Rc::new(value))),
                Err(error) => vm_err(error),
            },
            Expr::Undefined => Ok(Value::Undefined),
            Expr::Identifier(n) => {
                if n == "undefined" {
                    return Ok(Value::Undefined);
                }
                match self.global.borrow().lookup(n) {
                    Lookup::Value(v) => Ok(v),
                    // Declared in this block but the declaration has not run:
                    // the temporal dead zone. JavaScript distinguishes this
                    // from an undeclared name, and so do we.
                    Lookup::Uninitialized => vm_err(format!(
                        "ReferenceError: Cannot access '{}' before initialization",
                        n
                    )),
                    Lookup::Missing => vm_err(format!("ReferenceError: {} is not defined", n)),
                }
            }
            Expr::Array(i) => {
                let mut v = Vec::new();
                for x in i {
                    match x {
                        Expr::Spread(inner) => {
                            let inner_val = self.eval_expr(inner)?;
                            match &inner_val {
                                Value::Array(arr) => {
                                    let items = arr.borrow();
                                    if v.len().saturating_add(items.len())
                                        > crate::value::MAX_ARRAY_LEN
                                    {
                                        return Err(crate::value::limit_err(
                                            "Maximum array length exceeded",
                                        ));
                                    }
                                    v.extend(items.iter().cloned());
                                }
                                Value::String(s) => {
                                    if v.len().saturating_add(s.chars().count())
                                        > crate::value::MAX_ARRAY_LEN
                                    {
                                        return Err(crate::value::limit_err(
                                            "Maximum array length exceeded",
                                        ));
                                    }
                                    v.extend(s.chars().map(|c| Value::String(c.to_string())))
                                }
                                // Anything else goes through the iterator
                                // protocol — a generator, a typed array, an
                                // object with `Symbol.iterator`. Silently
                                // producing nothing here made `[...gen()]`
                                // return an empty array, so a value that is
                                // not iterable now says so.
                                other => {
                                    let items = self.drain_iterable(other)?;
                                    if v.len().saturating_add(items.len())
                                        > crate::value::MAX_ARRAY_LEN
                                    {
                                        return Err(crate::value::limit_err(
                                            "Maximum array length exceeded",
                                        ));
                                    }
                                    v.extend(items);
                                }
                            }
                        }
                        _ => v.push(self.eval_expr(x)?),
                    }
                    if v.len() > crate::value::MAX_ARRAY_LEN {
                        return Err(crate::value::limit_err("Maximum array length exceeded"));
                    }
                }
                Value::checked_array(v)
            }
            Expr::Object(props) => self.eval_object_literal(props),
            Expr::Binary { op, left, right } => {
                let l = self.eval_expr(left)?;
                match op {
                    crate::parser::BinOp::And if !self.truthy(&l) => return Ok(l),
                    crate::parser::BinOp::Or if self.truthy(&l) => return Ok(l),
                    crate::parser::BinOp::Nullish
                        if !matches!(l, Value::Null | Value::Undefined) =>
                    {
                        return Ok(l);
                    }
                    crate::parser::BinOp::And
                    | crate::parser::BinOp::Or
                    | crate::parser::BinOp::Nullish => return self.eval_expr(right),
                    _ => {}
                }
                let r = self.eval_expr(right)?;
                if matches!(op, crate::parser::BinOp::Instanceof) {
                    return self.instance_of(&l, &r);
                }
                // A proxy's `has` trap answers `in`. It runs guest code, so it
                // cannot live in `bin_op`, which does not borrow mutably.
                // `+` may need to run a guest `toString`, which `bin_op`
                // cannot do from `&self`. Coerce the operands first.
                if matches!(op, crate::parser::BinOp::Add)
                    && (Self::needs_concat_coercion(&l) || Self::needs_concat_coercion(&r))
                {
                    let left = self.coerce_for_concat(&l)?;
                    let right = self.coerce_for_concat(&r)?;
                    return self.bin_op(*op, &left, &right);
                }
                if matches!(op, crate::parser::BinOp::In)
                    && let Some(proxy) = r.as_proxy()
                {
                    let target = proxy.target.clone();
                    return match self.proxy_trap(&proxy, "has") {
                        Some(trap) => {
                            let key = self.proxy_property_key(&l)?;
                            let handler = proxy.handler.clone();
                            let result = self.call_this(&trap, handler, vec![target, key])?;
                            Ok(Value::Bool(result.is_truthy()))
                        }
                        None => self.bin_op(*op, &l, &target),
                    };
                }
                self.bin_op(*op, &l, &r)
            }
            Expr::Unary {
                op,
                operand,
                prefix,
            } => {
                // `delete` needs the *reference*, not the value: it removes a
                // slot from the receiver rather than computing anything from
                // the property it names.
                if matches!(op, UnOp::Delete) {
                    match operand.as_ref() {
                        Expr::Member {
                            object, property, ..
                        } => {
                            let obj = self.eval_expr(object)?;
                            let key = self.eval_expr(property)?;
                            return self.delete_member(&obj, &key);
                        }
                        Expr::OptionalChain {
                            object, property, ..
                        } => {
                            let obj = self.eval_expr(object)?;
                            if matches!(obj, Value::Null | Value::Undefined) {
                                return Ok(Value::Bool(true));
                            }
                            let key = self.eval_expr(property)?;
                            return self.delete_member(&obj, &key);
                        }
                        // `delete someBinding` is `false`: declared bindings
                        // are not configurable. An unresolvable name is not a
                        // reference at all, so it deletes vacuously — and does
                        // not raise the `ReferenceError` that reading it would.
                        Expr::Identifier(name) => {
                            let bound = self.global.borrow().get(name).is_some();
                            return Ok(Value::Bool(!bound));
                        }
                        // `delete 42`: not a reference, so nothing to remove.
                        other => {
                            self.eval_expr(other)?;
                            return Ok(Value::Bool(true));
                        }
                    }
                }
                if matches!(op, UnOp::Inc | UnOp::Dec)
                    && matches!(operand.as_ref(), Expr::Identifier(_) | Expr::Member { .. })
                {
                    match operand.as_ref() {
                        Expr::Identifier(n) => {
                            let inc = *op == UnOp::Inc;
                            // Fused read-modify-write: one `borrow_mut` + one
                            // scan instead of a read borrow followed by a
                            // separate write borrow. `old` captures the value
                            // before the update so postfix can return it.
                            let mut old = None;
                            let new_val = {
                                let mut env = self.global.borrow_mut();
                                env.modify(n, |cur| {
                                    let cur_num = self.tn(&cur);
                                    old = Some(cur);
                                    Value::Number(if inc { cur_num + 1.0 } else { cur_num - 1.0 })
                                })
                            };
                            let new_val = match new_val {
                                ModifyOutcome::Updated(v) => v,
                                ModifyOutcome::Missing => {
                                    return vm_err(format!("ReferenceError: {} is not defined", n));
                                }
                                ModifyOutcome::Const => {
                                    return vm_err(format!(
                                        "TypeError: Assignment to constant variable '{}'",
                                        n
                                    ));
                                }
                                ModifyOutcome::Uninitialized => {
                                    return vm_err(format!(
                                        "ReferenceError: Cannot access '{}' before initialization",
                                        n
                                    ));
                                }
                            };
                            if *prefix {
                                Ok(new_val)
                            } else {
                                Ok(old.unwrap_or(Value::Undefined))
                            }
                        }
                        Expr::Member {
                            object,
                            property,
                            computed: _,
                        } => {
                            let obj = self.eval_expr(object)?;
                            let prop = self.eval_expr(property)?;
                            let cur = self.get_prop_value(&obj, &prop)?;
                            let new_val = if *op == UnOp::Inc {
                                Value::Number(self.tn(&cur) + 1.0)
                            } else {
                                Value::Number(self.tn(&cur) - 1.0)
                            };
                            self.assign_member(&obj, &prop, new_val.clone())?;
                            if *prefix { Ok(new_val) } else { Ok(cur) }
                        }
                        _ => {
                            let v = self.eval_expr(operand)?;
                            self.un_op(*op, &v)
                        }
                    }
                } else if *op == UnOp::Typeof {
                    // `typeof` never throws, even on undeclared identifiers.
                    let v = if let Expr::Identifier(n) = operand.as_ref() {
                        if n == "undefined" {
                            Value::Undefined
                        } else {
                            self.global.borrow().get(n).unwrap_or(Value::Undefined)
                        }
                    } else {
                        self.eval_expr(operand)?
                    };
                    self.un_op(*op, &v)
                } else {
                    let v = self.eval_expr(operand)?;
                    self.un_op(*op, &v)
                }
            }
            Expr::Call { callee, args } => {
                let mut a = Vec::new();
                for x in args {
                    match x {
                        Expr::Spread(inner) => {
                            let inner_val = self.eval_expr(inner)?;
                            match &inner_val {
                                Value::Array(arr) => {
                                    let items = arr.borrow();
                                    if a.len().saturating_add(items.len())
                                        > crate::value::MAX_ARRAY_LEN
                                    {
                                        return Err(crate::value::limit_err(
                                            "Maximum argument count exceeded",
                                        ));
                                    }
                                    a.extend(items.iter().cloned());
                                }
                                _ => push_call_arg(&mut a, inner_val)?,
                            }
                        }
                        _ => push_call_arg(&mut a, self.eval_expr(x)?)?,
                    }
                }
                match callee.as_ref() {
                    // `super(...)` invokes the superclass constructor on the
                    // current `this`.
                    Expr::Super => {
                        let this_val = self.global.borrow().get("this").unwrap_or(Value::Undefined);
                        let super_ctor =
                            self.global.borrow().get("__super_ctor").ok_or_else(|| {
                                VmErr::Msg("super used outside a derived class".to_string())
                            })?;
                        self.invoke_ctor(&super_ctor, this_val, a)
                    }
                    // `super.m(...)`: the method comes from the superclass
                    // prototype, but `this` stays the current receiver.
                    Expr::Member {
                        object,
                        property,
                        computed: _,
                    } if matches!(object.as_ref(), Expr::Super) => {
                        let this_val = self.global.borrow().get("this").unwrap_or(Value::Undefined);
                        let prop = self.eval_expr(property)?;
                        let method = self.super_member(&prop)?;
                        self.call_this(&method, this_val, a)
                    }
                    // Method call: bind `this` to the receiver object.
                    Expr::Member {
                        object,
                        property,
                        computed: _,
                    } => {
                        let obj = self.eval_expr(object)?;
                        let prop = self.eval_expr(property)?;
                        let f = self.get_prop_value(&obj, &prop)?;
                        self.call_this(&f, obj, a)
                    }
                    Expr::OptionalChain {
                        object,
                        property,
                        computed: _,
                    } => {
                        let obj = self.eval_expr(object)?;
                        if matches!(obj, Value::Null | Value::Undefined) {
                            return Ok(Value::Undefined);
                        }
                        // A `Undefined` property marks an optional call `obj?.(args)`.
                        let f = if matches!(property.as_ref(), Expr::Undefined) {
                            obj.clone()
                        } else {
                            let prop = self.eval_expr(property)?;
                            self.get_prop_value(&obj, &prop)?
                        };
                        self.call_this(&f, obj, a)
                    }
                    _ => {
                        let c = self.eval_expr(callee)?;
                        self.call_this(&c, Value::Undefined, a)
                    }
                }
            }
            // `super.x` reads through the superclass prototype.
            Expr::Member {
                object,
                property,
                computed: _,
            } if matches!(object.as_ref(), Expr::Super) => {
                let p = self.eval_expr(property)?;
                self.super_member(&p)
            }
            Expr::Member {
                object,
                property,
                computed: _,
            } => {
                let o = self.eval_expr(object)?;
                let p = self.eval_expr(property)?;
                self.get_prop_value(&o, &p)
            }
            Expr::OptionalChain {
                object,
                property,
                computed: _,
            } => {
                let o = self.eval_expr(object)?;
                if matches!(o, Value::Null | Value::Undefined) {
                    return Ok(Value::Undefined);
                }
                let p = self.eval_expr(property)?;
                self.get_prop_value(&o, &p)
            }
            // `a &&= b` / `a ||= b` / `a ??= b`. The right side runs, and the
            // write happens, only when the current value calls for it — so
            // `obj.x ||= expensive()` leaves a truthy `x` untouched and never
            // evaluates `expensive`.
            Expr::LogicalAssignment { target, op, value } => {
                let (receiver, key, current) = match target.as_ref() {
                    Expr::Identifier(name) => {
                        let current = self.eval_expr(target)?;
                        let _ = name;
                        (None, None, current)
                    }
                    Expr::Member {
                        object, property, ..
                    } => {
                        let receiver = self.eval_expr(object)?;
                        let key = self.eval_expr(property)?;
                        let current = self.get_prop_value(&receiver, &key)?;
                        (Some(receiver), Some(key), current)
                    }
                    _ => return vm_err("Invalid assignment target"),
                };
                let should_assign = match op {
                    LogicalAssignOp::And => self.truthy(&current),
                    LogicalAssignOp::Or => !self.truthy(&current),
                    LogicalAssignOp::Nullish => {
                        matches!(current, Value::Null | Value::Undefined)
                    }
                };
                if !should_assign {
                    return Ok(current);
                }
                let assigned = self.eval_expr(value)?;
                match (receiver, key) {
                    (Some(receiver), Some(key)) => {
                        self.assign_member(&receiver, &key, assigned.clone())?;
                    }
                    _ => {
                        let Expr::Identifier(name) = target.as_ref() else {
                            unreachable!("checked above");
                        };
                        self.assign_or_set_binding(name, assigned.clone())?;
                    }
                }
                Ok(assigned)
            }
            Expr::Assignment { target, op, value } => {
                let v = self.eval_expr(value)?;
                match target.as_ref() {
                    Expr::Identifier(n) => {
                        let v = if matches!(op, AssignOp::Add) {
                            self.coerce_for_concat(&v)?
                        } else {
                            v
                        };
                        let fv = if let Some(bin) = op.bin_op() {
                            // Fused read-modify-write: one `borrow_mut` + one
                            // scan instead of a read borrow then a write borrow.
                            // `bin_op` can still fail (e.g. string-length cap);
                            // capture the error and leave the slot unchanged.
                            let mut err = None;
                            // An object on the *left* of `+=` needs its
                            // `toString`, which cannot run while the scope is
                            // borrowed. Detect it here and redo the whole
                            // assignment outside the borrow, leaving the slot
                            // untouched in the meantime.
                            let mut deferred = None;
                            let res = {
                                let mut env = self.global.borrow_mut();
                                env.modify(n, |cur| {
                                    if matches!(bin, crate::parser::BinOp::Add)
                                        && Self::needs_concat_coercion(&cur)
                                    {
                                        deferred = Some(cur.clone());
                                        return cur;
                                    }
                                    match self.bin_op(bin, &cur, &v) {
                                        Ok(new) => new,
                                        Err(e) => {
                                            err = Some(e);
                                            cur
                                        }
                                    }
                                })
                            };
                            if let Some(current) = deferred {
                                let left = self.coerce_for_concat(&current)?;
                                let combined = self.bin_op(bin, &left, &v)?;
                                self.assign_or_set_binding(n, combined.clone())?;
                                return Ok(combined);
                            }
                            if let Some(e) = err {
                                return Err(e);
                            }
                            match res {
                                ModifyOutcome::Updated(v) => v,
                                ModifyOutcome::Missing => {
                                    return vm_err(format!("ReferenceError: {} is not defined", n));
                                }
                                ModifyOutcome::Const => {
                                    return vm_err(format!(
                                        "TypeError: Assignment to constant variable '{}'",
                                        n
                                    ));
                                }
                                ModifyOutcome::Uninitialized => {
                                    return vm_err(format!(
                                        "ReferenceError: Cannot access '{}' before initialization",
                                        n
                                    ));
                                }
                            }
                        } else {
                            self.assign_or_set_binding(n, v.clone())?;
                            v
                        };
                        Ok(fv)
                    }
                    Expr::Member {
                        object,
                        property,
                        computed: _,
                    } => {
                        let obj = self.eval_expr(object)?;
                        let prop = self.eval_expr(property)?;
                        let fv = if let Some(bin) = op.bin_op() {
                            let c = self.get_prop_value(&obj, &prop)?;
                            self.bin_op(bin, &c, &v)?
                        } else {
                            v
                        };
                        self.assign_member(&obj, &prop, fv.clone())?;
                        Ok(fv)
                    }
                    // A destructuring *assignment*: `[a, b] = [b, a]`,
                    // `({ x } = o)`. Unlike a declaration it binds nothing
                    // new, so each name is assigned through the scope chain.
                    Expr::Array(_) | Expr::Object(_) if matches!(op, AssignOp::Assign) => {
                        let pattern = expr_to_pattern(target)
                            .ok_or_else(|| VmErr::Msg("Invalid assignment target".to_string()))?;
                        self.destructure(&pattern, &v)?;
                        Ok(v)
                    }
                    _ => vm_err("Invalid assignment target"),
                }
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                let t = self.eval_expr(test)?;
                if self.truthy(&t) {
                    self.eval_expr(consequent)
                } else {
                    self.eval_expr(alternate)
                }
            }
            // A class expression's own name is visible only inside its body,
            // which a child scope provides.
            Expr::ClassExpr {
                name,
                superclass,
                body,
            } => {
                let scope = Rc::new(RefCell::new(Environment::child(self.global.clone())));
                let saved = std::mem::replace(&mut self.global, scope);
                let built =
                    self.build_class(name.as_deref().unwrap_or(""), superclass.as_deref(), body);
                if let (Ok(value), Some(name)) = (&built, name) {
                    self.global.borrow_mut().set(name, value.clone());
                }
                self.global = saved;
                built
            }
            Expr::ArrowFn {
                params,
                body,
                is_async,
            } => Ok(Value::Function(Box::new(FunctionData {
                identity: Rc::new(0),
                name: None,
                properties: FunctionData::properties_with_default_prototype(
                    &self.persistent_global,
                ),
                standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                params: intern_params(params),
                closure: Some(self.global.clone()),
                uses_arguments: arrow_body_references(body, "arguments"),
                body: Rc::new(match body.as_ref() {
                    ExprOrBlock::Block(s) => s.clone(),
                    ExprOrBlock::Expr(e) => vec![Statement::Return(Some(e.clone()))],
                }),
                is_arrow: true,
                is_constructor: false,
                is_async: *is_async,
                is_generator: false,
                bound: None,
            }))),
            Expr::FnExpr {
                name,
                params,
                body,
                is_async,
                is_generator,
            } => Ok(Value::Function(Box::new(FunctionData {
                identity: Rc::new(0),
                name: name.as_deref().map(Rc::from),
                properties: FunctionData::properties_with_default_prototype(
                    &self.persistent_global,
                ),
                standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                params: intern_params(params),
                body: Rc::new(body.clone()),
                closure: Some(self.global.clone()),
                is_arrow: false,
                is_constructor: !*is_async && !*is_generator,
                is_async: *is_async,
                is_generator: *is_generator,
                uses_arguments: stmts_reference(body, "arguments"),
                bound: None,
            }))),
            Expr::New { callee, args } => {
                let mut a = Vec::new();
                for x in args {
                    push_call_arg(&mut a, self.eval_expr(x)?)?;
                }
                let c = self.eval_expr(callee)?;
                self.ctor(&c, a)
            }
            Expr::Spread(i) => self.eval_expr(i),
            Expr::This => Ok(self.global.borrow().get("this").unwrap_or(Value::Undefined)),
            // `import(specifier)`. Module registration is synchronous in this
            // VM, so the promise is already settled when it is handed back;
            // `await import(…)` and `.then(…)` both work.
            Expr::DynamicImport(specifier) => {
                let specifier = self.eval_expr(specifier)?;
                let name = self.vs(&specifier)?;
                let resolved = self.resolve_module_name(&name);
                if let Some(target) = resolved.as_ref() {
                    let target = target.clone();
                    self.ensure_module(&target)?;
                }
                match resolved.and_then(|n| self.module(&n)) {
                    Some(module) => Ok(Value::settled_promise(
                        PromiseState::Fulfilled,
                        Self::namespace_object(&module)?,
                    )),
                    None => Ok(Value::settled_promise(
                        PromiseState::Rejected,
                        Value::Error(crate::value::ErrorData::new(
                            "TypeError",
                            format!("Module not found: {}", name),
                        )),
                    )),
                }
            }
            Expr::ImportMeta => {
                let o = vec![
                    ("url".to_string(), Value::String("vm://module".to_string())),
                    ("main".to_string(), Value::Bool(self.is_main)),
                ];
                Ok(Value::object(o))
            }
            // `` tag`a${x}b` ``: the tag receives the literal chunks as an
            // array carrying a `raw` companion, then the interpolated values.
            Expr::TaggedTemplate {
                tag,
                cooked,
                raw,
                exprs,
            } => {
                let (this_val, tag_fn) = match tag.as_ref() {
                    // Preserve the receiver so `` obj.tag`…` `` sees `this`.
                    Expr::Member {
                        object, property, ..
                    } => {
                        let receiver = self.eval_expr(object)?;
                        let key = self.eval_expr(property)?;
                        let f = self.get_prop_value(&receiver, &key)?;
                        (receiver, f)
                    }
                    other => (Value::Undefined, self.eval_expr(other)?),
                };
                let strings = Value::array(cooked.iter().cloned().map(Value::String).collect());
                strings.set_prop(
                    "raw".to_string(),
                    Value::array(raw.iter().cloned().map(Value::String).collect()),
                )?;
                let mut args = vec![strings];
                for expr in exprs {
                    args.push(self.eval_expr(expr)?);
                }
                self.call_this(&tag_fn, this_val, args)
            }
            Expr::Template { quasis, exprs } => {
                let mut result = String::new();
                for (i, q) in quasis.iter().enumerate() {
                    if result.len().saturating_add(q.len()) > crate::value::MAX_STRING_LEN {
                        return Err(crate::value::limit_err("Maximum string length exceeded"));
                    }
                    result.push_str(q);
                    if i < exprs.len() {
                        let val = self.eval_expr(&exprs[i])?;
                        let rendered = self.display_string(&val)?;
                        if result.len().saturating_add(rendered.len())
                            > crate::value::MAX_STRING_LEN
                        {
                            return Err(crate::value::limit_err("Maximum string length exceeded"));
                        }
                        result.push_str(&rendered);
                    }
                }
                Value::checked_string(result)
            }
            Expr::Super => vm_err("'super' must be called as a function"),
            Expr::Await(inner) => {
                let v = self.eval_expr(inner)?;
                self.perform_await(v)
            }
            Expr::Yield(arg) => {
                // Evaluate the yielded expression, then switch back to whoever
                // called `next()`. Execution resumes here when `next(v)` is
                // called again, and `v` becomes the value of this expression.
                //
                // An abandoned generator is resumed once more with
                // `GenResume::Return`, so guest `finally` blocks still run.
                // Evaluated on every target: the expression may have side
                // effects even where suspension is unsupported.
                let v = match arg {
                    Some(e) => self.eval_expr(e)?,
                    None => Value::Undefined,
                };
                // Where suspension is unavailable, the value goes to the
                // buffer the driver drains, and the `yield` expression itself
                // evaluates to `undefined`.
                #[cfg(not(stackful_coroutines))]
                if let Some(sink) = self.yield_sink.as_ref() {
                    if sink.borrow().len() >= crate::value::MAX_ARRAY_LEN {
                        return Err(crate::value::limit_err("Maximum generator output exceeded"));
                    }
                    sink.borrow_mut().push(v);
                    return Ok(Value::Undefined);
                }
                #[cfg(not(stackful_coroutines))]
                let _ = v;
                #[cfg(stackful_coroutines)]
                if let Some(yielder) = self.gen_yielder.as_ref() {
                    return match yielder.suspend(v) {
                        crate::value::GenResume::Next(sent) => Ok(sent.unwrap_or(Value::Undefined)),
                        // `gen.throw(e)`: raise at the suspension point, so a
                        // `try`/`catch` around the `yield` sees it.
                        crate::value::GenResume::Throw(reason) => vm_throw(reason),
                        // Closed (`gen.return()` / leaving `for...of` early):
                        // return from the body so the surrounding
                        // `try`/`finally` still runs on the way out.
                        crate::value::GenResume::Return => vm_ret(Value::Undefined),
                        // Abandoned while suspended: unwind with no guest
                        // handlers at all (see `VmErr::Abandon`).
                        crate::value::GenResume::Abandon => Err(VmErr::Abandon),
                    };
                }
                // Outside a generator body: yield is a no-op returning undefined.
                Ok(Value::Undefined)
            }
            Expr::YieldFrom(inner) => {
                // `yield* it` re-yields every value `it` produces, then
                // evaluates to `it`'s own return value. Values sent in with
                // `next(v)` are forwarded to the delegate.
                let source = self.eval_expr(inner)?;
                let iterator = self.iterator_for(&source)?;
                let next_fn = self.prop(&iterator, &Value::String("next".to_string()))?;
                if matches!(next_fn, Value::Undefined) {
                    return vm_err("TypeError: yield* requires an iterable");
                }

                let mut sent = Value::Undefined;
                loop {
                    self.consume_loop()?;
                    let step = self.call_this(&next_fn, iterator.clone(), vec![sent])?;
                    let done = step.get_prop("done").map(|v| v.is_truthy()).unwrap_or(true);
                    let value = step.get_prop("value").unwrap_or(Value::Undefined);
                    if done {
                        // The delegate's return value is this expression's.
                        return Ok(value);
                    }

                    // `yield*` re-yields into the same buffer.
                    #[cfg(not(stackful_coroutines))]
                    {
                        if let Some(sink) = self.yield_sink.as_ref() {
                            if sink.borrow().len() >= crate::value::MAX_ARRAY_LEN {
                                return Err(crate::value::limit_err(
                                    "Maximum generator output exceeded",
                                ));
                            }
                            sink.borrow_mut().push(value);
                        }
                        sent = Value::Undefined;
                    }
                    #[cfg(stackful_coroutines)]
                    match self.gen_yielder.as_ref() {
                        Some(yielder) => match yielder.suspend(value) {
                            crate::value::GenResume::Next(v) => {
                                sent = v.unwrap_or(Value::Undefined);
                            }
                            crate::value::GenResume::Throw(reason) => {
                                close_iterator(&iterator);
                                return vm_throw(reason);
                            }
                            crate::value::GenResume::Return => {
                                // The outer generator is being closed: close
                                // the delegate too, then unwind.
                                close_iterator(&iterator);
                                return vm_ret(Value::Undefined);
                            }
                            crate::value::GenResume::Abandon => {
                                // Abandoning the outer generator: do *not*
                                // close the delegate (that would run its
                                // handlers). It is a local; dropping it on
                                // the way out abandons it in turn.
                                return Err(VmErr::Abandon);
                            }
                        },
                        // Outside a generator body there is nobody to yield
                        // to; drain the iterator for its side effects.
                        None => sent = Value::Undefined,
                    }
                }
            }
        }
    }
}
