//! Statement and expression evaluation: the two big `match` dispatchers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{
    BindKind, Env, Environment, Interpreter, Lookup, block_needs_lexical_scope, body_needs_hoisting,
};
use crate::error::{VmErr, vm_err, vm_ret, vm_throw};
use crate::parser::{
    AssignOp, ClassMember, Expr, ExprOrBlock, ForBinding, ForInit, LogicalAssignOp, MemberName,
    ObjectProp, Statement, UnOp, VarKind, arrow_body_references,
};
use crate::value::{ClassData, FunctionData, ObjectCell, PrivateElement, PropAttrs, Value};

/// Convert parser-owned parameter names into interned `Rc<str>` so call-frame
/// binding is a refcount bump, not a heap allocation.
pub(crate) fn intern_params(params: &[String]) -> Rc<Vec<Rc<str>>> {
    Rc::new(params.iter().map(|p| Rc::from(p.as_str())).collect())
}

/// Length-guard a static member key without allocating it. Evaluating a
/// string literal rejects oversized values, and the borrowed-key member
/// paths below preserve that error exactly.
#[inline]
fn checked_static_key(key: &str) -> Result<(), VmErr> {
    if key.len() > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    Ok(())
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

#[derive(Debug, Clone)]
pub(crate) enum ClassStaticElement {
    Field {
        name: Value,
        private: bool,
        init: Expr,
    },
    Block(Rc<Vec<Statement>>),
}

/// Evaluated class parts handed to [`Interpreter::assemble_class`]:
/// the constructor plus gathered prototype/static members, with static
/// blocks to run once the class value exists.
pub(crate) struct ClassAssembly {
    pub name: String,
    pub super_cls: Option<Value>,
    pub super_proto: Option<Rc<Value>>,
    pub constructor: Value,
    pub constructor_length: usize,
    pub proto_props: Vec<(String, Value)>,
    pub proto_keys: Vec<Value>,
    pub static_keys: Vec<Value>,
    pub statics: Vec<(String, Value)>,
    pub static_attrs: Vec<(String, PropAttrs)>,
    pub static_has_accessors: bool,
    pub static_elements: Vec<ClassStaticElement>,
    pub private_scope: Env,
    pub instance_home: Env,
    pub static_home: Env,
    pub constructor_home: Env,
    pub private_statics: Vec<(u64, PrivateElement)>,
}

/// Both class frontends register definitions here. Receiver branding and
/// accessor combination do not depend on AST or bytecode representation.
pub(crate) fn define_class_private_element(
    scope: &Env,
    statics: &mut Vec<(u64, PrivateElement)>,
    name: &str,
    is_static: bool,
    element: PrivateElement,
) -> Result<(), VmErr> {
    let id = scope
        .borrow()
        .private_name(name)
        .ok_or_else(|| VmErr::Msg("SyntaxError: private name is not declared".into()))?;
    if is_static {
        PrivateElement::define(statics, id, element)
    } else {
        scope
            .borrow_mut()
            .define_private_instance_element(id, element)
    }
}

pub(crate) fn insert_class_method(
    properties: &mut Vec<(String, Value)>,
    key: String,
    value: Value,
) {
    if let Some((_, existing)) = properties.iter_mut().find(|(name, _)| name == &key) {
        *existing = value;
    } else {
        properties.push((key.clone(), value));
    }
    let companion = format!("__setter:{key}__");
    properties.retain(|(name, _)| name != &companion);
}

pub(crate) fn insert_class_accessor(
    statics: &mut Vec<(String, Value)>,
    key: &str,
    accessor: Value,
) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectAccessorKind {
    Getter,
    Setter,
}

/// Add an object-literal property, replacing an earlier value for the same
/// key while preserving a getter/setter pair for accessor properties.
/// Shared by the AST evaluator and the bytecode VM's single-shot literal
/// construction, so both tiers lay out identical slots.
pub(crate) fn insert_object_property(
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

pub(crate) fn push_call_arg(args: &mut Vec<Value>, value: Value) -> Result<(), VmErr> {
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
/// Lexical home objects are guest-inaccessible environment slots. Their
/// prototypes are read at use time, rather than captured during definition.
pub(crate) const HOME_OBJECT: &str = "__home object__";
pub(crate) const SUPER_CONSTRUCTOR_HOME: &str = "__super constructor home__";

impl Interpreter {
    /// IteratorClose preserves an original throw, including errors raised by
    /// retrieving return. Other abrupt completions may be replaced by close.
    pub(crate) fn close_guest_iterator_for_abrupt(
        &mut self,
        iterator: &Value,
        asynchronous: bool,
        completion: &VmErr,
    ) -> Result<(), VmErr> {
        let result = self.close_guest_iterator(iterator, asynchronous);
        if matches!(
            completion,
            VmErr::Throw(_) | VmErr::Msg(_) | VmErr::RuntimeError(_)
        ) {
            Ok(())
        } else {
            result
        }
    }

    pub(crate) fn close_guest_iterator(
        &mut self,
        iterator: &Value,
        asynchronous: bool,
    ) -> Result<(), VmErr> {
        let Some(method) = self.get_method(iterator, &Value::String("return".into()))? else {
            return Ok(());
        };
        let mut result = self.call_this(&method, iterator.clone(), vec![])?;
        if asynchronous {
            result = self.perform_await(result)?;
        }
        if !super::call::is_js_object(&result) {
            return Err(VmErr::Msg(
                "TypeError: iterator return must return an object".into(),
            ));
        }
        Ok(())
    }

    /// IteratorComplete/GetValue with observable property access. Completed
    /// iterators do not read value; a missing done property means false.
    pub(crate) fn iterator_result_fields(
        &mut self,
        result: &Value,
    ) -> Result<(bool, Value), VmErr> {
        if !super::call::is_js_object(result) {
            return Err(VmErr::Msg(
                "TypeError: Iterator result must be an object".into(),
            ));
        }
        let done = self.member(result, "done")?.is_truthy();
        let value = if done {
            Value::Undefined
        } else {
            self.member(result, "value")?
        };
        Ok((done, value))
    }

    pub(crate) fn super_reference(
        &mut self,
        scope: &Env,
        key: &Value,
    ) -> Result<(Value, Value, Value), VmErr> {
        let receiver = self.resolve_this(scope)?;
        let key = self.ecmascript_to_property_key(key)?;
        let home = scope
            .borrow()
            .get(HOME_OBJECT)
            .ok_or_else(|| VmErr::Msg("ReferenceError: missing super home object".into()))?;
        let base = self.get_prototype_of(&home)?;
        Ok((base, receiver, key))
    }

    pub(crate) fn super_constructor(&mut self, scope: &Env) -> Result<Value, VmErr> {
        let home = scope.borrow().get(SUPER_CONSTRUCTOR_HOME).ok_or_else(|| {
            VmErr::Msg("ReferenceError: super() outside a derived constructor".into())
        })?;
        self.get_prototype_of(&home)
    }

    fn super_member(&mut self, key: &Value) -> Result<Value, VmErr> {
        let (base, receiver, key) = self.super_reference(&self.global.clone(), key)?;
        self.get_prop_value_with_receiver(&base, &key, &receiver)
    }

    /// Build a class value from its parts: prototype methods and accessors,
    /// static members, instance fields desugared into the constructor, and
    /// static blocks run once the class exists.
    ///
    /// Shared by the declaration and the class *expression*, which differ
    /// only in whether the result is bound to a name.
    pub(crate) fn evaluate_class_field(
        &mut self,
        initializer: &Expr,
        key: &Value,
    ) -> Result<Value, VmErr> {
        if let Expr::ClassExpr {
            name: None,
            superclass,
            body,
        } = initializer.unparenthesized()
        {
            let name = self.property_function_name(key)?;
            return self.build_class_with_name("", &name, superclass.as_deref(), body);
        }
        let value = self.eval_expr(initializer)?;
        if matches!(
            initializer.unparenthesized(),
            Expr::FnExpr { name: None, .. }
                | Expr::ArrowFn { .. }
                | Expr::ClassExpr { name: None, .. }
        ) {
            let name = self.property_function_name(key)?;
            if let Value::Function(function) = &value {
                let mut function = function.as_ref().clone();
                function.name = Some(Rc::from(name));
                return Ok(Value::Function(Rc::new(function)));
            }
        }
        Ok(value)
    }

    fn class_member_name(&mut self, scope: &Env, name: &MemberName) -> Result<Value, VmErr> {
        let saved = std::mem::replace(&mut self.global, scope.clone());
        let result = (|| match name {
            MemberName::Static(name) | MemberName::Private(name) => {
                Ok(Value::String(crate::JsString::from_key(name)))
            }
            MemberName::Computed(expr) => {
                let value = self.eval_expr(expr)?;
                self.ecmascript_to_property_key(&value)
            }
        })();
        self.global = saved;
        result
    }

    /// The prototype a class inherits from: the superclass's own, or a
    /// native constructor's `.prototype` property when that is an object.
    /// Shared with the bytecode class builder, which evaluates the
    /// superclass expression to a value first.
    pub(crate) fn super_proto_for(
        &mut self,
        super_cls: &Option<Value>,
    ) -> Result<Option<Rc<Value>>, VmErr> {
        let Some(superclass) = super_cls else {
            return Ok(None);
        };
        if matches!(superclass, Value::Null) {
            return Ok(Some(Rc::new(Value::Null)));
        }
        if !crate::builtins::is_constructor(superclass) {
            return Err(VmErr::Msg(
                "TypeError: class heritage is not a constructor or null".into(),
            ));
        }
        let prototype = self.get_prop_value_str(superclass, "prototype")?;
        if !matches!(prototype, Value::Null) && !super::call::is_js_object(&prototype) {
            return Err(VmErr::Msg(
                "TypeError: superclass prototype must be an object or null".into(),
            ));
        }
        Ok(Some(Rc::new(prototype)))
    }

    /// The `super(...)` target for a derived constructor: the superclass's
    /// constructor, or a callable native heritage itself.
    pub(crate) fn class_environment(parent: Env, name: &str) -> Env {
        let scope = Rc::new(RefCell::new(Environment::child(parent)));
        scope.borrow_mut().replace_strict(Some(true));
        if !name.is_empty() {
            scope
                .borrow_mut()
                .declare(name, Value::Uninitialized, BindKind::Const, false);
        }
        scope
    }

    pub(crate) fn object_method_with_home(value: &Value, home: &Env) -> Value {
        if let Value::Function(function) = value {
            let mut function = function.as_ref().clone();
            function.closure = Some(crate::heap::capture_env(home));
            Value::Function(Rc::new(function))
        } else {
            value.clone()
        }
    }

    pub(crate) fn initialize_object_home(home: &Env, object: &Value) {
        home.borrow_mut().set(HOME_OBJECT, object.clone());
    }

    /// PropertyDefinitionEvaluation's prototype setter ignores primitives and
    /// delegates actual prototype changes to the shared internal operation.
    pub(crate) fn initialize_literal_prototype(
        &mut self,
        object: &Value,
        prototype: &Value,
    ) -> Result<(), VmErr> {
        if (matches!(prototype, Value::Null) || super::call::is_js_object(prototype))
            && !self.set_prototype_of(object, prototype)?
        {
            return Err(VmErr::Msg(
                "TypeError: Cannot set object literal prototype".into(),
            ));
        }
        Ok(())
    }

    /// Each method group gets a home-object environment, including base
    /// classes. Assembly initializes the home once the object exists.
    pub(crate) fn member_closure_env(global: &Env) -> Env {
        Rc::new(RefCell::new(Environment::child(global.clone())))
    }

    /// Assemble a class value from evaluated parts: the constructor and
    /// the gathered prototype/static members. The member walk (name
    /// evaluation, function building, constructor detection) stays with
    /// each tier; everything from the prototype object on is shared.
    pub(crate) fn assemble_class(&mut self, asm: ClassAssembly) -> Result<Value, VmErr> {
        let ClassAssembly {
            name,
            super_cls,
            super_proto,
            constructor,
            constructor_length,
            proto_props,
            proto_keys,
            static_keys,
            mut statics,
            mut static_attrs,
            static_has_accessors,
            static_elements,
            private_scope,
            instance_home,
            static_home,
            constructor_home,
            private_statics,
        } = asm;
        let has_constructor_member = proto_props.iter().any(|(key, _)| key == "constructor");
        let prototype_has_accessors = proto_props.iter().any(|(key, value)| key.starts_with("__setter:") || matches!(value, Value::Function(function) if function.name.as_deref().is_some_and(|name| name.starts_with("get ") || name.starts_with("set "))));
        let super_proto = if super_cls.is_none() && super_proto.is_none() {
            self.persistent_global
                .borrow()
                .intrinsic("Object")
                .and_then(|object| object.get_prop("prototype"))
                .map(Rc::new)
        } else {
            super_proto
        };
        let prototype = Value::object_with_proto(proto_props, super_proto);
        if let Value::Object { props } = &prototype {
            let mut meta = props.meta.borrow_mut();
            meta.has_accessors = prototype_has_accessors;
            for (key, _) in props.borrow().iter() {
                meta.set_attrs(
                    key,
                    PropAttrs {
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
            for key in proto_keys {
                if let Value::Symbol(symbol) = &key {
                    meta.set_symbol_key(&super::symbol_slot_key(symbol), symbol.clone());
                }
            }
        }
        if !has_constructor_member {
            prototype.set_prop("constructor".to_string(), constructor.clone())?;
        }

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
        let static_properties =
            crate::heap::tracked(Rc::new(ObjectCell::new_with_default_proto(statics)));
        if let Some(superclass) = &super_cls
            && !matches!(superclass, Value::Null)
        {
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
            for key in static_keys {
                if let Value::Symbol(symbol) = &key {
                    meta.set_symbol_key(&super::symbol_slot_key(symbol), symbol.clone());
                }
            }
        }

        let class_val = Value::Class(Box::new(ClassData {
            name: name.to_string(),
            constructor: Box::new(constructor),
            prototype: Rc::new(prototype),
            statics: static_properties,
        }));
        if let Value::Class(class) = &class_val {
            if !has_constructor_member {
                class
                    .prototype
                    .as_ref()
                    .set_prop("constructor".to_owned(), class_val.clone())?;
            }
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

        if let Value::Class(class) = &class_val {
            instance_home
                .borrow_mut()
                .set(HOME_OBJECT, class.prototype.as_ref().clone());
            static_home.borrow_mut().set(HOME_OBJECT, class_val.clone());
            if super_cls.is_some() {
                constructor_home
                    .borrow_mut()
                    .set(SUPER_CONSTRUCTOR_HOME, class_val.clone());
            }
        }

        for (id, element) in private_statics {
            class_val.initialize_private_element(id, element)?;
        }

        if !name.is_empty() && private_scope.borrow().own_binding(&name).is_some() {
            private_scope
                .borrow_mut()
                .declare(&name, class_val.clone(), BindKind::Const, true);
        }
        // Static methods/accessors are already installed. Fields and blocks
        // initialize in source order after every computed name was evaluated.
        for element in static_elements {
            let scope = Rc::new(RefCell::new(Environment::function_child(
                private_scope.clone(),
            )));
            scope.borrow_mut().class_initializer = true;
            scope.borrow_mut().replace_strict(Some(true));
            scope.borrow_mut().set("this", class_val.clone());
            scope.borrow_mut().set_new_target(Value::Undefined);
            scope.borrow_mut().set(HOME_OBJECT, class_val.clone());
            let saved = std::mem::replace(&mut self.global, scope);
            let result = (|| {
                self.execution.check()?;
                match element {
                    ClassStaticElement::Block(block) => self.run_program_body(&block).map(|_| ()),
                    ClassStaticElement::Field {
                        name,
                        private,
                        init,
                    } => {
                        let value = self.evaluate_class_field(&init, &name)?;
                        if private {
                            let id = self
                                .global
                                .borrow()
                                .private_name(&self.property_key(&name)?)
                                .ok_or_else(|| {
                                    VmErr::Msg(
                                        "TypeError: missing private field declaration".into(),
                                    )
                                })?;
                            class_val.initialize_private_field(id, value)
                        } else {
                            let descriptor = Value::descriptor_record(vec![
                                ("value".into(), value),
                                ("writable".into(), Value::Bool(true)),
                                ("enumerable".into(), Value::Bool(true)),
                                ("configurable".into(), Value::Bool(true)),
                            ]);
                            if !self.define_own_property(&class_val, &name, &descriptor)? {
                                return Err(VmErr::Msg(
                                    "TypeError: Cannot define static field".into(),
                                ));
                            }
                            Ok(())
                        }
                    }
                }
            })();
            self.global = saved;
            result?;
        }
        Ok(class_val)
    }

    fn build_class(
        &mut self,
        name: &str,
        superclass: Option<&Expr>,
        body: &[ClassMember],
    ) -> Result<Value, VmErr> {
        self.build_class_with_name(name, name, superclass, body)
    }

    fn build_class_with_name(
        &mut self,
        binding_name: &str,
        name: &str,
        superclass: Option<&Expr>,
        body: &[ClassMember],
    ) -> Result<Value, VmErr> {
        let member_scope = Self::class_environment(self.global.clone(), binding_name);
        let saved = std::mem::replace(&mut self.global, member_scope.clone());
        let heritage = (|| {
            let superclass = superclass.map(|expr| self.eval_expr(expr)).transpose()?;
            let prototype = self.super_proto_for(&superclass)?;
            Ok::<_, VmErr>((superclass, prototype))
        })();
        self.global = saved;
        let (super_cls, super_proto) = heritage?;
        member_scope
            .borrow_mut()
            .declare_private_declarations(crate::parser::class_private_declarations(body));
        for member in body {
            if let ClassMember::Field {
                name: MemberName::Private(name),
                is_static: false,
                ..
            } = member
            {
                member_scope.borrow_mut().declare_private_field(name);
            }
        }
        let member_closure = Self::member_closure_env(&member_scope);
        let static_member_closure = Self::member_closure_env(&member_scope);

        // Gather the constructor, instance fields, and methods.
        let mut ctor_params: Vec<String> = Vec::new();
        let mut ctor_body: Vec<Statement> = Vec::new();
        let mut has_own_constructor = false;
        let mut instance_fields: Vec<(Expr, Option<Expr>, bool)> = Vec::new();
        let mut ctor_computed_keys = Vec::new();
        let mut proto_keys = Vec::new();
        let mut static_keys = Vec::new();
        let mut proto_props: Vec<(String, Value)> = Vec::new();
        let mut statics: Vec<(String, Value)> =
            vec![("name".to_string(), Value::String((name.to_string()).into()))];
        let mut static_attrs = vec![(
            "name".to_owned(),
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        )];
        let mut static_has_accessors = false;
        let mut private_statics = Vec::new();
        let mut static_elements = Vec::new();

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
                    let key = self.class_member_name(&member_scope, name)?;
                    let mname = self.property_key(&key)?;
                    let display_name = self.property_function_name(&key)?;
                    if !matches!(name, MemberName::Private(_)) {
                        if *st {
                            static_keys.push(key);
                        } else {
                            proto_keys.push(key);
                        }
                    }
                    // Only a written-out `constructor` is the constructor; a
                    // computed key that happens to evaluate to it stays an
                    // ordinary method.
                    let is_ctor_name = matches!(name, MemberName::Static(n) if n == "constructor");
                    let fn_val = Value::Function(Rc::new(FunctionData {
                        strict: true,
                        native: None,
                        identity: Rc::new(0),
                        name: Some(display_name.as_str().into()),
                        properties: FunctionData::properties_with_function_kind(
                            &self.persistent_global,
                            *is_async,
                            *is_generator,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: intern_params(mp),
                        body: Rc::new(mb.clone()),
                        closure: Some(crate::heap::capture_env(if *st {
                            &static_member_closure
                        } else {
                            &member_closure
                        })),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: *is_async,
                        is_generator: *is_generator,
                        uses_arguments: crate::parser::stmts_need_arguments(mb),
                        bytecode: None,
                        needs_hoisting: body_needs_hoisting(mb),
                        bound: None,
                    }));
                    if matches!(name, MemberName::Private(_)) {
                        define_class_private_element(
                            &member_scope,
                            &mut private_statics,
                            &mname,
                            *st,
                            PrivateElement::Method(fn_val),
                        )?;
                    } else if *st {
                        insert_class_method(&mut statics, mname.clone(), fn_val);
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
                        insert_class_method(&mut proto_props, mname.clone(), fn_val);
                    }
                }
                // Static blocks are collected and run after the class
                // exists, since they observe its statics and `this`.
                ClassMember::StaticBlock { body } => {
                    static_elements.push(ClassStaticElement::Block(Rc::new(body.clone())));
                }
                ClassMember::Field {
                    name,
                    is_static: st,
                    init,
                } => {
                    let fname = self.class_member_name(&member_scope, name)?;
                    if *st {
                        static_elements.push(ClassStaticElement::Field {
                            name: fname,
                            private: matches!(name, MemberName::Private(_)),
                            init: init.clone().unwrap_or(Expr::Undefined),
                        });
                    } else {
                        let property = if matches!(name, MemberName::Computed(_)) {
                            let index = ctor_computed_keys.len();
                            ctor_computed_keys.push(fname);
                            Expr::Identifier(crate::bytecode::constants::class_key_name(index))
                        } else {
                            Expr::String(crate::JsString::from_key(&self.property_key(&fname)?))
                        };
                        instance_fields.push((
                            property,
                            init.clone(),
                            !matches!(name, MemberName::Private(_)),
                        ));
                    }
                }
                ClassMember::Getter {
                    name,
                    is_static: st,
                    body: gb,
                } => {
                    let key = self.class_member_name(&member_scope, name)?;
                    let gname = self.property_key(&key)?;
                    let display_name = self.property_function_name(&key)?;
                    if !matches!(name, MemberName::Private(_)) {
                        if *st {
                            static_keys.push(key);
                        } else {
                            proto_keys.push(key);
                        }
                    }
                    let getter_fn = Value::Function(Rc::new(FunctionData {
                        strict: true,
                        native: None,
                        identity: Rc::new(0),
                        name: Some(format!("get {}", display_name).into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: Rc::new(vec![]),
                        body: Rc::new(gb.clone()),
                        closure: Some(crate::heap::capture_env(if *st {
                            &static_member_closure
                        } else {
                            &member_closure
                        })),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: false,
                        is_generator: false,
                        uses_arguments: crate::parser::stmts_need_arguments(gb),
                        bytecode: None,
                        needs_hoisting: body_needs_hoisting(gb),
                        bound: None,
                    }));
                    if matches!(name, MemberName::Private(_)) {
                        define_class_private_element(
                            &member_scope,
                            &mut private_statics,
                            &gname,
                            *st,
                            PrivateElement::Accessor {
                                get: Some(getter_fn),
                                set: None,
                            },
                        )?;
                    } else if *st {
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
                        insert_class_accessor(&mut proto_props, &gname, getter_fn);
                    }
                }
                ClassMember::Setter {
                    name,
                    param,
                    is_static: st,
                    body: sb,
                } => {
                    let key = self.class_member_name(&member_scope, name)?;
                    let sname = self.property_key(&key)?;
                    let display_name = self.property_function_name(&key)?;
                    if !matches!(name, MemberName::Private(_)) {
                        if *st {
                            static_keys.push(key);
                        } else {
                            proto_keys.push(key);
                        }
                    }
                    let setter_fn = Value::Function(Rc::new(FunctionData {
                        strict: true,
                        native: None,
                        identity: Rc::new(0),
                        name: Some(format!("set {}", display_name).into()),
                        properties: FunctionData::properties_with_default_prototype(
                            &self.persistent_global,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: Rc::new(vec![Rc::from(param.as_str())]),
                        body: Rc::new(sb.clone()),
                        closure: Some(crate::heap::capture_env(if *st {
                            &static_member_closure
                        } else {
                            &member_closure
                        })),
                        is_arrow: false,
                        is_constructor: false,
                        is_async: false,
                        is_generator: false,
                        uses_arguments: crate::parser::stmts_need_arguments(sb),
                        bytecode: None,
                        needs_hoisting: body_needs_hoisting(sb),
                        bound: None,
                    }));
                    if matches!(name, MemberName::Private(_)) {
                        define_class_private_element(
                            &member_scope,
                            &mut private_statics,
                            &sname,
                            *st,
                            PrivateElement::Accessor {
                                get: None,
                                set: Some(setter_fn),
                            },
                        )?;
                    } else if *st {
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
                        insert_class_accessor(&mut proto_props, &sname, setter_fn);
                    }
                }
            }
        }

        // A derived class with no constructor of its own gets the implicit
        // argument forwarding without guest iterator calls. Without it, extending a
        // class whose constructor does the work — `class E extends Error {}` —
        // produced an instance the superclass never initialized.
        if super_cls.is_some() && !has_own_constructor {
            ctor_params = vec!["...args".to_string()];
            ctor_body = Vec::new();
        }

        // Store fields separately from the body so constructor entry/super
        // controls their initialization rather than ordinary statement order.
        let mut full_ctor_body = Vec::new();
        for (fname, init, computed) in instance_fields {
            let value = init.unwrap_or(Expr::Undefined);
            full_ctor_body.push(Statement::Expr(Expr::Assignment {
                target: Box::new(Expr::Member {
                    object: Box::new(Expr::This),
                    property: Box::new(fname),
                    computed,
                }),
                op: AssignOp::Assign,
                value: Box::new(value),
            }));
        }
        let fields = full_ctor_body;
        let mut full_ctor_body = Vec::new();
        if super_cls.is_some()
            || !fields.is_empty()
            || !member_scope.borrow().private_instance_elements().is_empty()
        {
            full_ctor_body.push(Statement::ClassInitialization {
                derived: super_cls.is_some(),
                forward_rest: (super_cls.is_some() && !has_own_constructor).then(|| "args".into()),
                fields,
            });
        }
        full_ctor_body.extend(ctor_body);

        // Constructors retain their class home, so super() obtains the
        // current constructor prototype before evaluating its arguments.
        let ctor_closure = Rc::new(RefCell::new(Environment::child(member_closure.clone())));
        if super_cls.is_some() {
            ctor_closure
                .borrow_mut()
                .set(SUPER_CONSTRUCTOR_HOME, Value::Undefined);
        }
        for (index, key) in ctor_computed_keys.into_iter().enumerate() {
            ctor_closure
                .borrow_mut()
                .set(&crate::bytecode::constants::class_key_name(index), key);
        }

        let constructor_length =
            crate::parser::formal_parameter_length(&ctor_params, &full_ctor_body);
        let constructor = Value::Function(Rc::new(FunctionData {
            strict: true,
            native: None,
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
            uses_arguments: crate::parser::stmts_need_arguments(&full_ctor_body),
            needs_hoisting: body_needs_hoisting(&full_ctor_body),
            body: Rc::new(full_ctor_body),
            closure: Some(crate::heap::capture_env(&ctor_closure)),
            is_arrow: false,
            is_constructor: false,
            is_async: false,
            is_generator: false,
            bytecode: None,
            bound: None,
        }));

        self.assemble_class(ClassAssembly {
            name: name.to_string(),
            super_cls,
            super_proto,
            constructor,
            constructor_length,
            proto_props,
            proto_keys,
            static_keys,
            statics,
            static_attrs,
            static_has_accessors,
            private_scope: member_scope,
            instance_home: member_closure,
            static_home: static_member_closure,
            constructor_home: ctor_closure,
            private_statics,
            static_elements,
        })
    }

    /// Shared `import` statement: resolve and evaluate the module, then bind
    /// its exports as live cells. Both the AST evaluator and the register
    /// VM's `Import` instruction run this, so binding semantics stay single.
    pub(crate) fn stmt_import(
        &mut self,
        module: &str,
        default: Option<&str>,
        named: &[(String, String)],
        namespace: Option<&str>,
    ) -> Result<Value, VmErr> {
        let resolved_module = self.resolve_module_request(module)?;
        if let Some(name) = resolved_module.as_ref() {
            let name = name.clone();
            self.ensure_module(&name)?;
        }
        if let Some(md) = resolved_module.as_ref().and_then(|name| self.module(name)) {
            if let Some(d) = default {
                let v = md.default.clone().ok_or_else(|| {
                    VmErr::Msg(format!(
                        "SyntaxError: Module '{module}' has no default export"
                    ))
                })?;
                self.bind_import(d, v)?;
            }
            for (imported, local) in named {
                // `import { default as x }` names the default export.
                let v = if imported == "default" {
                    md.default.clone().ok_or_else(|| {
                        VmErr::Msg(format!(
                            "SyntaxError: Module '{module}' has no default export"
                        ))
                    })?
                } else {
                    match md.exports.get(imported).cloned() {
                        Some(entry) => entry,
                        None => {
                            return Err(VmErr::Msg(format!(
                                "SyntaxError: Module '{module}' has no unambiguous export '{imported}'"
                            )));
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
        } else if module.starts_with('.') && self.cur_mod.is_none() {
            vm_err(format!(
                "Relative import requires a module context: {}",
                module
            ))
        } else {
            vm_err(format!("Module not found: {}", module))
        }
    }

    /// Shared `export default`: publish one value as the default export.
    pub(crate) fn stmt_export_default(&mut self, value: Value) -> Result<Value, VmErr> {
        let mut record = self.current_module();
        if let Some(Value::Binding(cell)) = &record.default {
            *cell.borrow_mut() = value;
        } else {
            record.default = Some(value);
        }
        Ok(Value::Undefined)
    }

    /// Shared `export { ... }`, with or without a `from` source.
    pub(crate) fn stmt_export_named(
        &mut self,
        specifiers: &[(String, String)],
        source: Option<&str>,
    ) -> Result<Value, VmErr> {
        if self.is_linked_module() {
            return Ok(Value::Undefined);
        }
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
                    for ((local, exported), (_, existing)) in specifiers.iter().zip(promised) {
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

    /// Shared `export * [as ns] from 'm'`.
    pub(crate) fn stmt_export_all(
        &mut self,
        source: &str,
        alias: Option<&str>,
    ) -> Result<Value, VmErr> {
        if self.is_linked_module() {
            return Ok(Value::Undefined);
        }
        let resolved = self
            .resolve_module_request(source)?
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
                    .insert(alias.to_string(), namespace);
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

    /// Shared dynamic `import(specifier)`: a promise for the namespace.
    pub(crate) fn eval_dynamic_import(&mut self, specifier: Value) -> Result<Value, VmErr> {
        self.eval_dynamic_import_with_options(specifier, None)
    }

    fn eval_dynamic_import_with_options(
        &mut self,
        specifier: Value,
        options: Option<Value>,
    ) -> Result<Value, VmErr> {
        let target = Value::pending_promise();
        let converted = (|| {
            let name = self.ecmascript_to_string(&specifier)?.to_utf8().map_err(|_| VmErr::Msg("TypeError: module specifier contains an unpaired surrogate unsupported by the UTF-8 loader contract".into()))?;
            if let Some(options) = options.filter(|value| !matches!(value, Value::Undefined)) {
                if !super::call::is_js_object(&options) {
                    return Err(VmErr::Msg(
                        "TypeError: import options must be an object".into(),
                    ));
                }
                let attributes = self.get_prop_value_str(&options, "with")?;
                if !matches!(attributes, Value::Undefined) {
                    if !super::call::is_js_object(&attributes) {
                        return Err(VmErr::Msg(
                            "TypeError: import attributes must be an object".into(),
                        ));
                    }
                    let keys = crate::builtins::object::own_names_for(self, &attributes, true)?;
                    for key in &keys {
                        if !matches!(self.get_prop_value_str(&attributes, key)?, Value::String(_)) {
                            return Err(VmErr::Msg(
                                "TypeError: import attribute values must be strings".into(),
                            ));
                        }
                    }
                    if !keys.is_empty() {
                        return Err(VmErr::Msg(
                            "TypeError: import attributes are not supported by this module loader"
                                .into(),
                        ));
                    }
                }
            }
            Ok(name)
        })();
        match converted {
            Ok(specifier) => {
                self.jobs
                    .borrow_mut()
                    .push_microtask(super::jobs::Job::DynamicImport {
                        realm: self.persistent_global.clone(),
                        target: target.clone(),
                        specifier,
                        referrer: self.cur_mod.clone(),
                    })
            }
            Err(error) => self.reject_promise(
                &target,
                match error {
                    VmErr::Throw(reason) => reason,
                    other => crate::error::error_value_from_msg(&other.to_string()),
                },
            ),
        }
        Ok(Value::Promise(target))
    }

    /// Shared `import.meta`: the current module's URL and main flag.
    pub(crate) fn eval_import_meta(&mut self) -> Result<Value, VmErr> {
        let url = self
            .cur_mod
            .as_ref()
            .and_then(|module| self.module_file_urls.borrow().get(module).cloned())
            .unwrap_or_else(|| "vm://module".into());
        let o = vec![
            ("url".to_string(), Value::String((url).into())),
            ("main".to_string(), Value::Bool(self.is_main)),
        ];
        Ok(Value::object(o))
    }

    pub(super) fn eval_stmt(&mut self, s: &Statement) -> Result<Value, VmErr> {
        self.consume_fuel(1)?;
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
                                self.initialize_pattern_binding(pat, &v, bind_kind)?;
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
                ..
            } => {
                let scope = if self.global.borrow().is_eval_scope() {
                    Environment::variable_environment(&self.global)
                } else {
                    self.global.clone()
                };
                self.declare_function_binding_in(
                    &scope,
                    name,
                    Value::Function(Rc::new(FunctionData {
                        strict: self.global.borrow().strict() || crate::parser::use_strict(body),
                        native: None,
                        identity: Rc::new(0),
                        name: Some(name.as_str().into()),
                        properties: FunctionData::properties_with_function_kind(
                            &self.persistent_global,
                            *is_async,
                            *is_generator,
                        ),
                        standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                        params: intern_params(params),
                        body: Rc::new(body.clone()),
                        closure: Some(crate::heap::capture_env(&self.global)),
                        is_arrow: false,
                        is_constructor: !*is_async && !*is_generator,
                        is_async: *is_async,
                        is_generator: *is_generator,
                        uses_arguments: crate::parser::stmts_need_arguments(body),
                        bytecode: None,
                        needs_hoisting: body_needs_hoisting(body),
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
                    Some(ex) => {
                        let value = self.eval_expr(ex)?;
                        self.prepare_return_value(value)?
                    }
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
            Statement::ResourceForOf { name, iter, .. } => {
                let mut environment = Environment::child(self.global.clone());
                environment.declare(name, Value::Undefined, BindKind::Const, false);
                let saved = std::mem::replace(&mut self.global, Rc::new(RefCell::new(environment)));
                let source = self.eval_expr(iter);
                self.global = saved;
                source?;
                vm_err("TypeError: resource disposal execution is not implemented")
            }
            Statement::ResourceDeclaration { declarations, .. } => {
                // Initializers precede acquisition of a disposal method and
                // can fail against the already-instantiated lexical TDZ.
                if let Some(Statement::VarDecl {
                    init: Some(init), ..
                }) = declarations.first()
                {
                    self.eval_expr(init)?;
                }
                vm_err("TypeError: resource disposal execution is not implemented")
            }
            Statement::With { object, body } => {
                let object = self.eval_expr(object)?;
                if matches!(object, Value::Null | Value::Undefined) {
                    return vm_err("TypeError: with object is null or undefined");
                }
                let object = if super::call::is_js_object(&object) {
                    object
                } else {
                    let constructor = self
                        .persistent_global
                        .borrow()
                        .intrinsic("Object")
                        .expect("Object intrinsic");
                    self.call_this(&constructor, Value::Undefined, vec![object])?
                };
                let mut environment = Environment::child(self.global.clone());
                environment.with_object = Some(object);
                let scope = Rc::new(RefCell::new(environment));
                let saved = std::mem::replace(&mut self.global, scope);
                let result = self.run_block(body);
                self.global = saved;
                result
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
            Statement::ForIn { binding, obj, body } => {
                self.with_loop_binding_scope(binding, |vm| vm.run_for_in(binding, obj, body))
            }
            Statement::ForOf {
                binding,
                iter,
                body,
                is_await,
            } => self.with_loop_binding_scope(binding, |vm| {
                vm.run_for_of(binding, iter, body, *is_await)
            }),
            Statement::Block(s) => self.run_block(s),
            // A declarator group shares the enclosing scope: no new frame.
            Statement::Declarations(s) => self.run(s),
            Statement::ParameterInitialization { .. } | Statement::ClassInitialization { .. } => {
                Err(VmErr::Msg(
                    "Invalid parameter initialization position".into(),
                ))
            }
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
                        let value = re.guest_value();
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
                self.stmt_export_default(v)
            }
            Statement::ExportNamed {
                specifiers, source, ..
            } => self.stmt_export_named(specifiers, source.as_deref()),
            Statement::ExportAll { source, alias, .. } => {
                self.stmt_export_all(source, alias.as_deref())
            }
            Statement::Import {
                module,
                default,
                named,
                namespace,
                ..
            } => self.stmt_import(module, default.as_deref(), named, namespace.as_deref()),
            Statement::Empty => Ok(Value::Undefined),
        }
    }

    /// Collect every value an iterable produces, for spread and rest.
    ///
    /// Bounded by the loop budget and the array-length cap, so an infinite
    /// generator raises a catchable `RangeError` instead of hanging.
    pub(crate) fn drain_iterable(&mut self, source: &Value) -> Result<Vec<Value>, VmErr> {
        let iterator = self.iterator_for(source)?;
        self.drain_iterator(&iterator)
    }

    pub(crate) fn drain_iterator(&mut self, iterator: &Value) -> Result<Vec<Value>, VmErr> {
        let next_fn = self.member(iterator, "next")?;
        let mut out = Vec::new();
        loop {
            self.consume_loop()?;
            let step = self.call_this(&next_fn, iterator.clone(), vec![])?;
            let (done, value) = self.iterator_result_fields(&step)?;
            if done {
                return Ok(out);
            }
            out.push(value);
            if out.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
        }
    }

    /// Array and argument spread use the same iterator protocol in both tiers.
    pub(crate) fn append_iterable(
        &mut self,
        output: &mut Vec<Value>,
        source: &Value,
        limit_message: &str,
    ) -> Result<(), VmErr> {
        let items = self.drain_iterable(source)?;
        if output.len().saturating_add(items.len()) > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err(limit_message));
        }
        output.extend(items);
        Ok(())
    }

    /// Obtain an iterator for `for await (… of source)`.
    ///
    /// `Symbol.asyncIterator` wins when the source has one; otherwise the
    /// synchronous iterator is used and each of its values is awaited, which
    /// is how `for await` consumes an array of promises.
    fn async_iterator_for(&mut self, source: &Value) -> Result<Value, VmErr> {
        let key = crate::builtins::well_known("asyncIterator")
            .unwrap_or(Value::String(("Symbol.asyncIterator".to_string()).into()));
        if let Some(async_iter_fn) = self.get_method(source, &key)? {
            let iterator = self.call_this(&async_iter_fn, source.clone(), vec![])?;
            if !super::call::is_js_object(&iterator) {
                return vm_err("TypeError: Async iterator method must return an object");
            }
            return Ok(iterator);
        }
        let iterator = self.iterator_for(source)?;
        super::async_from_sync::create(self, iterator)
    }

    /// Obtain an iterator for `source`, following the `Symbol.iterator`
    /// protocol. Shared by `for...of` and `yield*`.
    pub(crate) fn iterator_for(&mut self, source: &Value) -> Result<Value, VmErr> {
        let key = crate::builtins::well_known("iterator").expect("Symbol.iterator");
        let Some(method) = self.get_method(source, &key)? else {
            return vm_err("TypeError: Value has no callable Symbol.iterator");
        };
        self.iterator_from_method(source, &method)
    }

    pub(crate) fn iterator_from_method(
        &mut self,
        source: &Value,
        method: &Value,
    ) -> Result<Value, VmErr> {
        let iterator = self.call_this(method, source.clone(), vec![])?;
        if !super::call::is_js_object(&iterator) {
            return vm_err("TypeError: Iterator method must return an object");
        }
        Ok(iterator)
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

    fn object_literal_callable(
        &self,
        home: &Env,
        name: &str,
        params: &[String],
        body: &[Statement],
        is_async: bool,
        is_generator: bool,
    ) -> Value {
        Value::Function(Rc::new(FunctionData {
            strict: self.global.borrow().strict() || crate::parser::use_strict(body),
            native: None,
            identity: Rc::new(0),
            name: Some(name.into()),
            properties: FunctionData::properties_with_function_kind(
                &self.persistent_global,
                is_async,
                is_generator,
            ),
            standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
            params: intern_params(params),
            body: Rc::new(body.to_vec()),
            closure: Some(crate::heap::capture_env(home)),
            is_arrow: false,
            is_constructor: false,
            is_async,
            is_generator,
            uses_arguments: crate::parser::stmts_need_arguments(body),
            bytecode: None,
            needs_hoisting: body_needs_hoisting(body),
            bound: None,
        }))
    }

    /// Capture the callee and receiver before arguments can mutate bindings,
    /// properties or with records. None is an optional-chain short circuit.
    fn evaluate_call_reference(&mut self, callee: &Expr) -> Result<Option<(Value, Value)>, VmErr> {
        match callee.unparenthesized() {
            Expr::Member {
                object,
                property,
                computed,
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    let key = self.eval_expr(property)?;
                    let (base, receiver, key) = self.super_reference(&self.global.clone(), &key)?;
                    let function = self.get_prop_value_with_receiver(&base, &key, &receiver)?;
                    return Ok(Some((function, receiver)));
                }
                let receiver = self.eval_expr(object)?;
                let function = if let Expr::String(key) = property.as_ref() {
                    checked_static_key(key)?;
                    if !computed && key.to_key().starts_with('#') {
                        self.get_private_member(&receiver, &key.to_key())?
                    } else {
                        self.get_prop_value_str(&receiver, &key.to_key())?
                    }
                } else {
                    let key = self.eval_expr(property)?;
                    self.get_prop_value(&receiver, &key)?
                };
                Ok(Some((function, receiver)))
            }
            Expr::OptionalChain {
                object, property, ..
            } => {
                if matches!(property.as_ref(), Expr::Undefined) {
                    let Some((function, receiver)) = self.evaluate_call_reference(object)? else {
                        return Ok(None);
                    };
                    return Ok((!matches!(function, Value::Null | Value::Undefined))
                        .then_some((function, receiver)));
                }
                let receiver = self.eval_expr(object)?;
                if matches!(receiver, Value::Null | Value::Undefined) {
                    return Ok(None);
                }
                let key = self.eval_expr(property)?;
                let function = self.get_prop_value(&receiver, &key)?;
                Ok(Some((function, receiver)))
            }
            other => {
                let function = self.eval_expr(other)?;
                let receiver = if let Expr::Identifier(name) = other {
                    self.with_binding_object(&self.global.clone(), name)?
                        .filter(|object| {
                            !matches!(object, Value::RealmGlobal(_) | Value::GlobalObject)
                        })
                        .unwrap_or(Value::Undefined)
                } else {
                    Value::Undefined
                };
                Ok(Some((function, receiver)))
            }
        }
    }

    fn evaluate_call_arguments(&mut self, args: &[Expr]) -> Result<Vec<Value>, VmErr> {
        let mut values = Vec::new();
        for argument in args {
            match argument {
                Expr::Spread(inner) => {
                    let value = self.eval_expr(inner)?;
                    self.append_iterable(&mut values, &value, "Maximum argument count exceeded")?;
                }
                _ => push_call_arg(&mut values, self.eval_expr(argument)?)?,
            }
        }
        Ok(values)
    }

    fn reject_call_assignment_target(&mut self, target: &Expr) -> Result<(), VmErr> {
        if matches!(
            target.unparenthesized(),
            Expr::Call { .. } | Expr::TaggedTemplate { .. }
        ) {
            self.eval_expr(target)?;
            return Err(VmErr::Msg(
                "ReferenceError: invalid assignment target".into(),
            ));
        }
        Ok(())
    }

    fn run_for_in(
        &mut self,
        binding: &ForBinding,
        obj: &Expr,
        body: &[Statement],
    ) -> Result<Value, VmErr> {
        if let ForBinding::Declaration {
            pattern,
            initializer: Some(value),
            ..
        } = binding
        {
            let value = self.eval_expr(value)?;
            self.destructure(pattern, &value)?;
        }
        let o = self.eval_expr(obj)?;
        let ks = self.keys_with_proxy_trap(&o)?;
        let body_needs_scope = block_needs_lexical_scope(body);
        let mut r = Value::Undefined;
        let label = self.active_label.take();
        for k in ks {
            self.consume_loop()?;
            self.enter_iteration_binding_scope(binding);
            self.assign_iteration_binding(binding, &Value::String(k.into()))?;
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

    fn run_for_of(
        &mut self,
        binding: &ForBinding,
        iter: &Expr,
        body: &[Statement],
        is_await: bool,
    ) -> Result<Value, VmErr> {
        let source = self.eval_expr(iter)?;
        let body_needs_scope = block_needs_lexical_scope(body);
        let iterator = if is_await {
            self.async_iterator_for(&source)?
        } else {
            self.iterator_for(&source)?
        };
        let next_fn = self.member(&iterator, "next")?;
        if matches!(next_fn, Value::Undefined) {
            return vm_err("TypeError: iterator has no next() method");
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
            if is_await {
                result = self.perform_await(result)?;
            }
            let (done, value) = self.iterator_result_fields(&result)?;
            if done {
                exhausted = true;
                break;
            }
            self.enter_iteration_binding_scope(binding);
            if let Err(error) = self.assign_iteration_binding(binding, &value) {
                if !error.is_abandon() {
                    let _ = self.close_guest_iterator(&iterator, is_await);
                }
                return Err(error);
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
                        self.close_guest_iterator_for_abrupt(&iterator, is_await, &error)?;
                    }
                    return Err(error);
                }
                Ok(value) => r = value,
            }
        }
        if !exhausted {
            self.close_guest_iterator(&iterator, is_await)?;
        }
        Ok(r)
    }

    fn with_loop_binding_scope<R>(
        &mut self,
        binding: &ForBinding,
        operation: impl FnOnce(&mut Self) -> Result<R, VmErr>,
    ) -> Result<R, VmErr> {
        if !matches!(
            binding,
            ForBinding::Declaration {
                kind: VarKind::Let | VarKind::Const,
                ..
            }
        ) {
            return operation(self);
        }
        let outer = self.global.clone();
        self.enter_iteration_binding_scope_from(binding, outer.clone());
        let result = operation(self);
        self.global = outer;
        result
    }

    fn enter_iteration_binding_scope(&mut self, binding: &ForBinding) {
        if matches!(
            binding,
            ForBinding::Declaration {
                kind: VarKind::Let | VarKind::Const,
                ..
            }
        ) {
            let outer = self
                .global
                .borrow()
                .parent_env()
                .expect("loop lexical environment");
            self.enter_iteration_binding_scope_from(binding, outer);
        }
    }

    fn enter_iteration_binding_scope_from(&mut self, binding: &ForBinding, outer: Env) {
        let ForBinding::Declaration { pattern, kind, .. } = binding else {
            unreachable!()
        };
        let scope = Rc::new(RefCell::new(Environment::child(outer)));
        let kind = if *kind == VarKind::Const {
            BindKind::Const
        } else {
            BindKind::Let
        };
        for name in crate::parser::pattern_names(pattern) {
            scope
                .borrow_mut()
                .declare(&name, Value::Undefined, kind, false);
        }
        self.global = scope;
    }

    fn assign_iteration_binding(
        &mut self,
        binding: &ForBinding,
        value: &Value,
    ) -> Result<Value, VmErr> {
        match binding {
            ForBinding::Assignment(target) => {
                self.reject_call_assignment_target(target)?;
                let pattern = crate::parser::expr_to_pattern(target)
                    .ok_or_else(|| VmErr::Msg("Invalid iteration assignment target".into()))?;
                self.destructure_assignment(&pattern, value)
            }
            ForBinding::Declaration {
                pattern: crate::parser::Pattern::Ident(name),
                kind: VarKind::Var,
                ..
            } => {
                self.assign_or_set_binding(name, value.clone())?;
                Ok(value.clone())
            }
            ForBinding::Declaration {
                pattern: crate::parser::Pattern::Ident(name),
                ..
            } => {
                self.set_binding(name, value.clone())?;
                Ok(value.clone())
            }
            ForBinding::Declaration {
                kind: VarKind::Var,
                pattern,
                ..
            } => self.destructure_assignment(pattern, value),
            ForBinding::Declaration { pattern, kind, .. } => {
                let kind = if *kind == VarKind::Const {
                    BindKind::Const
                } else {
                    BindKind::Let
                };
                self.initialize_pattern_binding(pattern, value, kind)
            }
        }
    }

    fn eval_object_literal(&mut self, props: &[ObjectProp]) -> Result<Value, VmErr> {
        let home = Self::member_closure_env(&self.global);
        let mut object = Vec::new();
        let mut positions = HashMap::new();
        let mut accessors = HashMap::new();
        let mut symbol_keys = Vec::new();
        let mut prototype = None;
        for prop in props {
            match prop {
                ObjectProp::CoverInitializedName { .. } => {
                    return vm_err("invalid object literal cover grammar");
                }
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
                ObjectProp::KeyValue(key, expression) if key == "__proto__" => {
                    prototype = Some(self.eval_expr(expression)?);
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
                ObjectProp::ComputedMethod {
                    key,
                    params,
                    body,
                    is_async,
                    is_generator,
                    is_getter,
                    is_setter,
                } => {
                    let key_value = self.eval_expr(key)?;
                    let property_key = self.ecmascript_to_property_key(&key_value)?;
                    let key = self.property_key(&property_key)?;
                    let accessor = if *is_getter {
                        Some(ObjectAccessorKind::Getter)
                    } else if *is_setter {
                        Some(ObjectAccessorKind::Setter)
                    } else {
                        None
                    };
                    let display = self.property_function_name(&property_key)?;
                    let function_name = if *is_getter {
                        format!("get {display}")
                    } else if *is_setter {
                        format!("set {display}")
                    } else {
                        display
                    };
                    let function = self.object_literal_callable(
                        &home,
                        &function_name,
                        params,
                        body,
                        *is_async,
                        *is_generator,
                    );
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        key.clone(),
                        function,
                        accessor,
                    );
                    if let Value::Symbol(symbol) = &property_key {
                        symbol_keys.push((key, symbol.clone()));
                    }
                }
                ObjectProp::Computed(key_expression, value_expression) => {
                    let key_value = self.eval_expr(key_expression)?;
                    let key_value = self.ecmascript_to_property_key(&key_value)?;
                    let symbol = match &key_value {
                        Value::Symbol(symbol) => Some(symbol.clone()),
                        _ => None,
                    };
                    let key = self.property_key(&key_value)?;
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
                    let function = self.object_literal_callable(
                        &home,
                        name,
                        params,
                        body,
                        *is_async,
                        *is_generator,
                    );
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
                    let function = self.object_literal_callable(
                        &home,
                        &format!("get {name}"),
                        &[],
                        body,
                        false,
                        false,
                    );
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
                    let function = self.object_literal_callable(
                        &home,
                        &format!("set {name}"),
                        std::slice::from_ref(param),
                        body,
                        false,
                        false,
                    );
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
                    self.for_each_spread_entry(&value, |key, property_value| {
                        insert_object_property(
                            &mut object,
                            &mut positions,
                            &mut accessors,
                            key,
                            property_value,
                            None,
                        );
                        if positions.len() > crate::value::MAX_OBJECT_PROPS {
                            return Err(crate::value::limit_err(
                                "Maximum object property count exceeded",
                            ));
                        }
                        Ok(())
                    })?;
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
        if let Some(prototype) = prototype {
            self.initialize_literal_prototype(&result, &prototype)?;
        }
        Self::initialize_object_home(&home, &result);
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
        self.consume_fuel(1)?;
        match e {
            Expr::LegacyLiteral(inner) | Expr::Parenthesized(inner) => self.eval_expr(inner),
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::String(s) | Expr::EscapedString(s) => {
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
                let scope = self.global.clone();
                match self.lookup_binding_in(&scope, n)? {
                    Lookup::Value(v) => Ok(v),
                    // Declared in this block but the declaration has not run:
                    // the temporal dead zone. JavaScript distinguishes this
                    // from an undeclared name, and so do we.
                    Lookup::Uninitialized => vm_err(format!(
                        "ReferenceError: Cannot access '{}' before initialization",
                        n
                    )),
                    Lookup::Missing if n == "undefined" => Ok(Value::Undefined),
                    Lookup::Missing => vm_err(format!("ReferenceError: {} is not defined", n)),
                }
            }
            Expr::Array { items: i, .. } => {
                let mut v = Vec::new();
                for x in i {
                    match x {
                        Expr::Spread(inner) => {
                            let inner_val = self.eval_expr(inner)?;
                            self.append_iterable(
                                &mut v,
                                &inner_val,
                                "Maximum array length exceeded",
                            )?;
                        }
                        _ => v.push(self.eval_expr(x)?),
                    }
                    if v.len() > crate::value::MAX_ARRAY_LEN {
                        return Err(crate::value::limit_err("Maximum array length exceeded"));
                    }
                }
                Value::checked_array(v)
            }
            Expr::Object { props, .. } => self.eval_object_literal(props),
            Expr::Binary { op, left, right } => {
                if *op == crate::parser::BinOp::In
                    && let Expr::Identifier(name) = left.as_ref()
                    && name.starts_with('#')
                {
                    let receiver = self.eval_expr(right)?;
                    return self.has_private_member(&receiver, name);
                }
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
                self.apply_binary(*op, &l, &r)
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
                    if let Expr::Member {
                        object, property, ..
                    } = operand.unparenthesized()
                        && matches!(object.as_ref(), Expr::Super)
                    {
                        let key = self.eval_expr(property)?;
                        self.super_reference(&self.global.clone(), &key)?;
                        return Err(VmErr::Msg(
                            "ReferenceError: Cannot delete a super property".into(),
                        ));
                    }
                    match operand.unparenthesized() {
                        Expr::Member {
                            object, property, ..
                        } => {
                            let obj = self.eval_expr(object)?;
                            let key = self.eval_expr(property)?;
                            let strict = self.global.borrow().strict();
                            return self.delete_member_or_throw(&obj, &key, strict);
                        }
                        Expr::OptionalChain {
                            object, property, ..
                        } => {
                            let obj = self.eval_expr(object)?;
                            if matches!(obj, Value::Null | Value::Undefined) {
                                return Ok(Value::Bool(true));
                            }
                            let key = self.eval_expr(property)?;
                            let strict = self.global.borrow().strict();
                            return self.delete_member_or_throw(&obj, &key, strict);
                        }
                        // Resolve object and declarative bindings through the
                        // shared environment operation used by both tiers.
                        Expr::Identifier(name) => {
                            let scope = self.global.clone();
                            return self.delete_binding_in(&scope, name);
                        }
                        // `delete 42`: not a reference, so nothing to remove.
                        other => {
                            self.eval_expr(other)?;
                            return Ok(Value::Bool(true));
                        }
                    }
                }
                if matches!(op, UnOp::Inc | UnOp::Dec) {
                    self.reject_call_assignment_target(operand)?;
                }
                if matches!(op, UnOp::Inc | UnOp::Dec)
                    && matches!(
                        operand.unparenthesized(),
                        Expr::Identifier(_) | Expr::Member { .. }
                    )
                {
                    if let Expr::Member {
                        object, property, ..
                    } = operand.unparenthesized()
                        && matches!(object.as_ref(), Expr::Super)
                    {
                        let key = self.eval_expr(property)?;
                        let (base, receiver, key) =
                            self.super_reference(&self.global.clone(), &key)?;
                        let current = self.get_prop_value_with_receiver(&base, &key, &receiver)?;
                        let (current, updated) = self.numeric_update(&current, *op == UnOp::Inc)?;
                        let strict = self.global.borrow().strict();
                        self.assign_property_with_receiver(
                            &base,
                            &key,
                            updated.clone(),
                            &receiver,
                            strict,
                        )?;
                        return Ok(if *prefix { updated } else { current });
                    }
                    match operand.unparenthesized() {
                        Expr::Identifier(n) => {
                            self.inc_global_binding(n, *op == UnOp::Inc, *prefix)
                        }
                        Expr::Member {
                            object,
                            property,
                            computed,
                        } => {
                            let obj = self.eval_expr(object)?;
                            if let Expr::String(key) = property.as_ref() {
                                checked_static_key(key)?;
                                let private = !computed && key.to_key().starts_with('#');
                                let cur = if private {
                                    self.get_private_member(&obj, &key.to_key())?
                                } else {
                                    self.get_prop_value_str(&obj, &key.to_key())?
                                };
                                let new_val = if *op == UnOp::Inc {
                                    Value::Number(self.tn(&cur) + 1.0)
                                } else {
                                    Value::Number(self.tn(&cur) - 1.0)
                                };
                                if private {
                                    self.set_private_member(&obj, &key.to_key(), new_val.clone())?;
                                } else {
                                    self.assign_member_str(&obj, &key.to_key(), new_val.clone())?;
                                }
                                return if *prefix { Ok(new_val) } else { Ok(cur) };
                            }
                            let prop = self.eval_expr(property)?;
                            self.inc_prop_value(&obj, &prop, *op == UnOp::Inc, *prefix)
                        }
                        _ => {
                            let v = self.eval_expr(operand)?;
                            self.un_op(*op, &v)
                        }
                    }
                } else if *op == UnOp::Typeof {
                    // `typeof` never throws, even on undeclared identifiers.
                    let v = if let Expr::Identifier(n) = operand.unparenthesized() {
                        if n == "undefined" {
                            Value::Undefined
                        } else {
                            {
                                let scope = self.global.clone();
                                match self.lookup_binding_in(&scope, n)? {
                                    Lookup::Value(value) => value,
                                    Lookup::Missing => Value::Undefined,
                                    Lookup::Uninitialized => {
                                        return vm_err(format!(
                                            "ReferenceError: Cannot access {n} before initialization"
                                        ));
                                    }
                                }
                            }
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
                if matches!(callee.unparenthesized(), Expr::Super) {
                    let target = self.super_constructor(&self.global.clone())?;
                    let args = self.evaluate_call_arguments(args)?;
                    return self.invoke_ctor(&target, Value::Undefined, args);
                }
                let direct_eval =
                    matches!(callee.unparenthesized(), Expr::Identifier(name) if name == "eval");
                let Some((function, receiver)) = self.evaluate_call_reference(callee)? else {
                    return Ok(Value::Undefined);
                };
                let arguments = self.evaluate_call_arguments(args)?;
                if direct_eval
                    && crate::builtins::is_intrinsic_eval(&function, &self.persistent_global)
                {
                    crate::builtins::eval_direct(self, arguments)
                } else {
                    self.call_this(&function, receiver, arguments)
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
                computed,
            } => {
                let o = self.eval_expr(object)?;
                if let Expr::String(key) = property.as_ref() {
                    checked_static_key(key)?;
                    if !computed && key.to_key().starts_with('#') {
                        return self.get_private_member(&o, &key.to_key());
                    }
                    if !computed
                        && key.to_key().starts_with('#')
                        && !self.has_property(&o, &Value::String(key.clone()))?
                    {
                        return vm_err("TypeError: receiver does not contain the private member");
                    }
                    return self.get_prop_value_str(&o, &key.to_key());
                }
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
                if let Expr::String(key) = property.as_ref() {
                    checked_static_key(key)?;
                    return self.get_prop_value_str(&o, &key.to_key());
                }
                let p = self.eval_expr(property)?;
                self.get_prop_value(&o, &p)
            }
            // `a &&= b` / `a ||= b` / `a ??= b`. The right side runs, and the
            // write happens, only when the current value calls for it — so
            // `obj.x ||= expensive()` leaves a truthy `x` untouched and never
            // evaluates `expensive`.
            Expr::LogicalAssignment { target, op, value } => {
                self.reject_call_assignment_target(target)?;
                if let Expr::Member {
                    object, property, ..
                } = target.unparenthesized()
                    && matches!(object.as_ref(), Expr::Super)
                {
                    let key = self.eval_expr(property)?;
                    let (base, receiver, key) = self.super_reference(&self.global.clone(), &key)?;
                    let current = self.get_prop_value_with_receiver(&base, &key, &receiver)?;
                    let should_assign = match op {
                        LogicalAssignOp::And => current.is_truthy(),
                        LogicalAssignOp::Or => !current.is_truthy(),
                        LogicalAssignOp::Nullish => {
                            matches!(current, Value::Null | Value::Undefined)
                        }
                    };
                    if !should_assign {
                        return Ok(current);
                    }
                    let assigned = self.eval_expr(value)?;
                    let strict = self.global.borrow().strict();
                    self.assign_property_with_receiver(
                        &base,
                        &key,
                        assigned.clone(),
                        &receiver,
                        strict,
                    )?;
                    return Ok(assigned);
                }

                // Static member target: skip the key allocation on both the
                // read and the conditional write.
                if let Expr::Member {
                    object,
                    property,
                    computed,
                } = target.unparenthesized()
                    && let Expr::String(key) = property.as_ref()
                {
                    checked_static_key(key)?;
                    let receiver = self.eval_expr(object)?;
                    let private = !computed && key.to_key().starts_with('#');
                    let current = if private {
                        self.get_private_member(&receiver, &key.to_key())?
                    } else {
                        self.get_prop_value_str(&receiver, &key.to_key())?
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
                    if private {
                        self.set_private_member(&receiver, &key.to_key(), assigned.clone())?;
                    } else {
                        self.assign_member_str(&receiver, &key.to_key(), assigned.clone())?;
                    }
                    return Ok(assigned);
                }
                let (receiver, key, current) = match target.unparenthesized() {
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
                        let Expr::Identifier(name) = target.unparenthesized() else {
                            unreachable!("checked above");
                        };
                        self.assign_or_set_binding(name, assigned.clone())?;
                    }
                }
                Ok(assigned)
            }
            Expr::Assignment { target, op, value } => {
                self.reject_call_assignment_target(target)?;
                if let Expr::Member {
                    object, property, ..
                } = target.unparenthesized()
                    && matches!(object.as_ref(), Expr::Super)
                {
                    let key = self.eval_expr(property)?;
                    let (base, receiver, key) = self.super_reference(&self.global.clone(), &key)?;
                    let current = op
                        .bin_op()
                        .map(|_| self.get_prop_value_with_receiver(&base, &key, &receiver))
                        .transpose()?;
                    let mut assigned = self.eval_expr(value)?;
                    if let Some(op) = op.bin_op() {
                        assigned = self.bin_op(op, &current.expect("compound value"), &assigned)?;
                    }
                    let strict = self.global.borrow().strict();
                    self.assign_property_with_receiver(
                        &base,
                        &key,
                        assigned.clone(),
                        &receiver,
                        strict,
                    )?;
                    return Ok(assigned);
                }
                let v = self.eval_expr(value)?;
                match target.unparenthesized() {
                    Expr::Identifier(n) => {
                        if op.bin_op().is_some() {
                            self.compound_assign_global(n, *op, v)
                        } else {
                            self.assign_or_set_binding(n, v.clone())?;
                            Ok(v)
                        }
                    }
                    Expr::Member {
                        object,
                        property,
                        computed,
                    } => {
                        let obj = self.eval_expr(object)?;
                        if let Expr::String(key) = property.as_ref() {
                            checked_static_key(key)?;
                            let private = !computed && key.to_key().starts_with('#');
                            let fv = if let Some(bin) = op.bin_op() {
                                let c = if private {
                                    self.get_private_member(&obj, &key.to_key())?
                                } else {
                                    self.get_prop_value_str(&obj, &key.to_key())?
                                };
                                self.bin_op(bin, &c, &v)?
                            } else {
                                v
                            };
                            if private {
                                self.set_private_member(&obj, &key.to_key(), fv.clone())?;
                            } else {
                                self.assign_member_str(&obj, &key.to_key(), fv.clone())?;
                            }
                            return Ok(fv);
                        }
                        let prop = self.eval_expr(property)?;
                        if let Some(bin) = op.bin_op() {
                            self.compound_assign_prop(&obj, &prop, bin, v)
                        } else {
                            self.assign_member(&obj, &prop, v.clone())?;
                            Ok(v)
                        }
                    }
                    // A destructuring *assignment*: `[a, b] = [b, a]`,
                    // `({ x } = o)`. Unlike a declaration it binds nothing
                    // new, so each name is assigned through the scope chain.
                    Expr::Array { .. } | Expr::Object { .. } | Expr::LegacyLiteral(_)
                        if matches!(op, AssignOp::Assign) =>
                    {
                        let pattern = crate::parser::expr_to_pattern(target)
                            .ok_or_else(|| VmErr::Msg("Invalid assignment target".to_string()))?;
                        self.destructure_assignment(&pattern, &v)?;
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
            } => self.build_class(name.as_deref().unwrap_or(""), superclass.as_deref(), body),
            Expr::ArrowFn {
                params,
                body,
                is_async,
            } => Ok(Value::Function(Rc::new(FunctionData {
                strict: self.global.borrow().strict()
                    || matches!(body.as_ref(), ExprOrBlock::Block(s) if crate::parser::use_strict(s)),
                native: None,
                identity: Rc::new(0),
                name: None,
                properties: FunctionData::properties_with_function_kind(
                    &self.persistent_global,
                    *is_async,
                    false,
                ),
                standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                params: intern_params(params),
                closure: Some(crate::heap::capture_env(&self.global)),
                uses_arguments: arrow_body_references(body, "arguments"),
                // An expression body declares nothing; only block bodies can.
                needs_hoisting: matches!(body.as_ref(), ExprOrBlock::Block(s) if body_needs_hoisting(s)),
                body: Rc::new(match body.as_ref() {
                    ExprOrBlock::Block(s) => s.clone(),
                    ExprOrBlock::Expr(e) => vec![Statement::Return(Some(e.clone()))],
                }),
                is_arrow: true,
                is_constructor: false,
                is_async: *is_async,
                is_generator: false,
                bytecode: None,
                bound: None,
            }))),
            Expr::FnExpr {
                name,
                params,
                body,
                is_async,
                is_generator,
            } => {
                let closure =
                    Environment::named_function_scope(self.global.clone(), name.as_deref());
                let function = Value::Function(Rc::new(FunctionData {
                    strict: self.global.borrow().strict() || crate::parser::use_strict(body),
                    native: None,
                    identity: Rc::new(0),
                    name: name.as_deref().map(Rc::from),
                    properties: FunctionData::properties_with_function_kind(
                        &self.persistent_global,
                        *is_async,
                        *is_generator,
                    ),
                    standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
                    params: intern_params(params),
                    body: Rc::new(body.clone()),
                    closure: Some(crate::heap::capture_env(&closure)),
                    is_arrow: false,
                    is_constructor: !*is_async && !*is_generator,
                    is_async: *is_async,
                    is_generator: *is_generator,
                    uses_arguments: crate::parser::stmts_need_arguments(body),
                    bytecode: None,
                    needs_hoisting: body_needs_hoisting(body),
                    bound: None,
                }));
                Environment::initialize_function_name(&closure, name.as_deref(), &function);
                Ok(function)
            }
            Expr::New { callee, args } => {
                let constructor = self.eval_expr(callee)?;
                let arguments = self.evaluate_call_arguments(args)?;
                self.ctor(&constructor, arguments)
            }
            Expr::Spread(i) => self.eval_expr(i),
            Expr::This => self.resolve_this(&self.global),
            // `import(specifier)`. Module registration is synchronous in this
            // VM, so the promise is already settled when it is handed back;
            // `await import(…)` and `.then(…)` both work.
            Expr::DynamicImport {
                specifier,
                options,
                phase,
            } => {
                if *phase != crate::parser::ImportPhase::Evaluation {
                    return vm_err("TypeError: non-evaluation import phases are not implemented");
                }
                let specifier = self.eval_expr(specifier)?;
                let options = options
                    .as_deref()
                    .map(|expr| self.eval_expr(expr))
                    .transpose()?;
                self.eval_dynamic_import_with_options(specifier, options)
            }
            Expr::ImportMeta => self.eval_import_meta(),
            Expr::NewTarget => self
                .global
                .borrow()
                .new_target()
                .ok_or_else(|| VmErr::Msg("SyntaxError: new.target outside a function".into())),
            // `` tag`a${x}b` ``: the tag receives the literal chunks as an
            // array carrying a `raw` companion, then the interpolated values.
            Expr::TaggedTemplate {
                tag,
                cooked,
                raw,
                exprs,
            } => {
                let (this_val, tag_fn) = match tag.unparenthesized() {
                    Expr::Member {
                        object, property, ..
                    } if matches!(object.as_ref(), Expr::Super) => {
                        let key = self.eval_expr(property)?;
                        let (base, receiver, key) =
                            self.super_reference(&self.global.clone(), &key)?;
                        let method = self.get_prop_value_with_receiver(&base, &key, &receiver)?;
                        (receiver, method)
                    }
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
                let strings = Value::array(
                    cooked
                        .iter()
                        .cloned()
                        .map(|part| part.map(Value::String).unwrap_or(Value::Undefined))
                        .collect(),
                );
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
                let mut result = crate::JsString::default();
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
                #[cfg(stackful_coroutines)]
                let v = if self.await_yielder.is_some() {
                    self.perform_await(v)?
                } else {
                    v
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
                        crate::value::GenResume::Return(value) => {
                            let value = self.prepare_return_value(value)?;
                            vm_ret(value)
                        }
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
                #[cfg(stackful_coroutines)]
                let asynchronous = self.await_yielder.is_some();
                #[cfg(not(stackful_coroutines))]
                let asynchronous = false;
                let iterator = if asynchronous {
                    self.async_iterator_for(&source)?
                } else {
                    self.iterator_for(&source)?
                };
                let next_fn = self.member(&iterator, "next")?;
                if matches!(next_fn, Value::Undefined) {
                    return vm_err("TypeError: yield* requires an iterable");
                }

                let mut received = crate::value::GenResume::Next(Some(Value::Undefined));
                loop {
                    self.consume_loop()?;
                    let returning = matches!(&received, crate::value::GenResume::Return(_));
                    let (method, args) = match received {
                        crate::value::GenResume::Next(value) => {
                            (next_fn.clone(), vec![value.unwrap_or(Value::Undefined)])
                        }
                        crate::value::GenResume::Return(value) => {
                            let method = self.iterator_method(&iterator, "return")?;
                            let Some(method) = method else {
                                return vm_ret(self.prepare_return_value(value)?);
                            };
                            (method, vec![value])
                        }
                        crate::value::GenResume::Throw(value) => {
                            let method = self.iterator_method(&iterator, "throw")?;
                            let Some(method) = method else {
                                self.close_guest_iterator(&iterator, asynchronous)?;
                                return vm_err("TypeError: Delegated iterator has no throw method");
                            };
                            (method, vec![value])
                        }
                        crate::value::GenResume::Abandon => return Err(VmErr::Abandon),
                    };
                    let step = self.call_this(&method, iterator.clone(), args)?;
                    let step = if asynchronous {
                        self.perform_await(step)?
                    } else {
                        step
                    };
                    if !super::call::is_js_object(&step) {
                        return vm_err("TypeError: Iterator result must be an object");
                    }
                    let done = self.member(&step, "done")?.is_truthy();
                    let value = self.member(&step, "value")?;
                    if done {
                        return if returning {
                            vm_ret(self.prepare_return_value(value)?)
                        } else {
                            Ok(value)
                        };
                    }
                    let value = if asynchronous {
                        self.perform_await(value)?
                    } else {
                        value
                    };

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
                        received = crate::value::GenResume::Next(None);
                    }
                    #[cfg(stackful_coroutines)]
                    match self.gen_yielder.as_ref() {
                        Some(yielder) => received = yielder.suspend(value),
                        // Outside a generator body there is nobody to yield
                        // to; drain the iterator for its side effects.
                        None => received = crate::value::GenResume::Next(None),
                    }
                }
            }
        }
    }

    /// GetMethod for iterator completion forwarding. Only nullish methods are
    /// absent; getters and non-callable values must produce observable errors.
    fn iterator_method(&mut self, iterator: &Value, name: &str) -> Result<Option<Value>, VmErr> {
        self.get_method(iterator, &Value::String(name.into()))
    }
}
