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
    ObjectProp, Pattern, Statement, UnOp, VarKind, arrow_body_references,
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

pub(super) fn decorator_add_initializer(
    _: &mut Interpreter,
    state: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if !matches!(state.get_prop("active"), Some(Value::Bool(true))) {
        return Err(VmErr::Msg(
            "TypeError: decorator application has finished".into(),
        ));
    }
    let initializer = args.first().cloned().unwrap_or(Value::Undefined);
    if !super::is_callable_value(&initializer) {
        return Err(VmErr::Msg("TypeError: initializer must be callable".into()));
    }
    let list = state
        .get_prop("initializers")
        .and_then(|list| list.as_array())
        .expect("internal initializer list");
    list.store_index(list.array_length(), initializer);
    Ok(Value::Undefined)
}

#[derive(Debug, Clone)]
pub(crate) enum ClassStaticElement {
    Field {
        decoration: Option<Value>,
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
    pub field_definitions: Vec<super::ClassFieldDefinition>,
    pub method_definitions: Vec<super::ClassMethodDefinition>,
    pub decorators: Vec<(Value, Value)>,
    pub source_text: Option<crate::parser::SourceTextRef>,
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
    pub(crate) fn cached_template_object(&self, site: u64) -> Option<Value> {
        let realm =
            super::realm::allocation_global().unwrap_or_else(|| self.persistent_global.clone());
        realm.borrow().template_objects.get(&site).cloned()
    }

    /// A template site belongs to a parsed source node; arrays belong to its
    /// executing realm and must survive function recreation and collection.
    pub(crate) fn cache_template_object(&mut self, site: u64, cooked: Value, raw: Value) -> Value {
        let realm =
            super::realm::allocation_global().unwrap_or_else(|| self.persistent_global.clone());
        if let Some(existing) = realm.borrow().template_objects.get(&site).cloned() {
            return existing;
        }
        if let Value::Array(array) = &cooked {
            array.finish_template(raw);
        }
        realm
            .borrow_mut()
            .template_objects
            .insert(site, cooked.clone());
        cooked
    }

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
        let home = scope
            .borrow()
            .get(HOME_OBJECT)
            .ok_or_else(|| VmErr::Msg("ReferenceError: missing super home object".into()))?;
        let base = self.get_prototype_of(&home)?;
        // Capture the super base before user code in ToPropertyKey can change
        // the home object's prototype. The reference keeps that base for both
        // the read and write of a compound assignment or update.
        let key = self.ecmascript_to_property_key(key)?;
        Ok((base, receiver, key))
    }

    pub(super) fn evaluate_super_property_key(&mut self, property: &Expr) -> Result<Value, VmErr> {
        // A derived constructor must have a bound receiver before the key
        // expression can run (including a nested super() call).
        self.resolve_this(&self.global.clone())?;
        self.eval_expr(property)
    }

    pub(crate) fn super_constructor(&mut self, scope: &Env) -> Result<Value, VmErr> {
        let home = scope.borrow().get(SUPER_CONSTRUCTOR_HOME).ok_or_else(|| {
            VmErr::Msg("ReferenceError: super() outside a derived constructor".into())
        })?;
        self.get_prototype_of(&home)
    }

    /// Build a class value from its parts: prototype methods and accessors,
    /// static members, instance fields desugared into the constructor, and
    /// static blocks run once the class exists.
    ///
    /// Shared by the declaration and the class *expression*, which differ
    /// only in whether the result is bound to a name.
    pub(crate) fn evaluate_private_class_field(
        &mut self,
        initializer: &Expr,
        key: &Value,
    ) -> Result<Value, VmErr> {
        let original_name = if let Value::String(storage) = key {
            let storage = storage.to_key_cow();
            if storage.starts_with("#\0auto-accessor:") {
                match self.global.borrow().lookup(&storage) {
                    super::Lookup::Value(value) => Some(value),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        let key = original_name.as_ref().unwrap_or(key);
        self.evaluate_class_field(initializer, key)
    }

    pub(crate) fn evaluate_class_field(
        &mut self,
        initializer: &Expr,
        key: &Value,
    ) -> Result<Value, VmErr> {
        if let Expr::ClassExpr {
            decorators,
            source_text: _,
            name: None,
            superclass,
            body,
        } = initializer.unparenthesized()
        {
            let name = self.property_function_name(key)?;
            return self.build_class_with_name(
                "",
                &name,
                superclass.as_deref(),
                body,
                decorators,
                initializer.source_text(),
            );
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

    pub(crate) fn check_static_method_key(key: &Value) -> Result<(), VmErr> {
        if matches!(key, Value::String(name) if name == "prototype") {
            return Err(VmErr::Msg(
                "TypeError: Cannot redefine class prototype".into(),
            ));
        }
        Ok(())
    }

    /// Assemble a class value from evaluated parts: the constructor and
    /// the gathered prototype/static members. The member walk (name
    /// evaluation, function building, constructor detection) stays with
    /// each tier; everything from the prototype object on is shared.
    pub(crate) fn assemble_class(&mut self, asm: ClassAssembly) -> Result<Value, VmErr> {
        let ClassAssembly {
            mut field_definitions,
            mut method_definitions,
            decorators,
            source_text,
            name,
            super_cls,
            super_proto,
            constructor,
            constructor_length,
            mut proto_props,
            proto_keys,
            static_keys,
            mut statics,
            mut static_attrs,
            static_has_accessors,
            mut static_elements,
            private_scope,
            instance_home,
            static_home,
            constructor_home,
            mut private_statics,
        } = asm;
        let has_constructor_member = proto_props.iter().any(|(key, _)| key == "constructor");
        let constructor_property =
            if let Some(index) = proto_props.iter().position(|(key, _)| key == "constructor") {
                proto_props.remove(index)
            } else {
                ("constructor".into(), constructor.clone())
            };
        proto_props.insert(0, constructor_property);
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

        // Built-in constructor properties precede user definitions. Existing
        // methods named length/name replace the values without moving them.
        let mut ordered = vec![
            (
                "length".to_owned(),
                Value::Number(constructor_length as f64),
            ),
            ("name".to_owned(), Value::String(name.clone().into())),
            ("prototype".to_owned(), prototype.clone()),
        ];
        for (key, value) in statics {
            if let Some((_, existing)) = ordered.iter_mut().find(|(name, _)| name == &key) {
                *existing = value;
            } else {
                ordered.push((key, value));
            }
        }
        statics = ordered;
        static_attrs.insert(
            0,
            (
                "length".to_owned(),
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            ),
        );
        static_attrs.insert(
            0,
            (
                "prototype".to_owned(),
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            ),
        );
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

        let mut class_val = Value::Class(Box::new(ClassData {
            name: name.to_string(),
            constructor: Box::new(constructor),
            prototype: Rc::new(prototype),
            statics: static_properties,
        }))
        .with_function_source(source_text.as_ref());
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

        let static_method_initializers = self.decorate_class_methods(
            &class_val,
            &private_scope,
            &mut method_definitions,
            &mut field_definitions,
            &mut private_statics,
        )?;

        let field_states = self.decorate_class_fields(&private_scope, &field_definitions)?;
        for (index, state) in field_states {
            let Some(ClassStaticElement::Field { decoration, .. }) = static_elements.get_mut(index)
            else {
                return Err(VmErr::Msg(
                    "InternalError: invalid decorated static field index".into(),
                ));
            };
            *decoration = Some(state);
        }

        for (id, element) in private_statics {
            class_val.initialize_private_element(id, element)?;
        }

        let original_class = class_val.clone();
        let extra_initializers = (!decorators.is_empty()).then(|| Value::array(Vec::new()));
        for (decorator, receiver) in decorators.into_iter().rev() {
            let state = Value::object(vec![
                ("active".into(), Value::Bool(true)),
                (
                    "initializers".into(),
                    extra_initializers
                        .as_ref()
                        .expect("decorated class")
                        .clone(),
                ),
            ]);
            let add_initializer = crate::builtins::bound_native_method(
                "addInitializer",
                1,
                decorator_add_initializer,
                None,
                state.clone(),
            );
            let context = Value::object(vec![
                ("kind".into(), Value::String("class".into())),
                ("name".into(), Value::String(name.as_str().into())),
                ("addInitializer".into(), add_initializer),
            ]);
            let result = self.call_this(&decorator, receiver, vec![class_val.clone(), context]);
            state.set_prop("active".into(), Value::Bool(false))?;
            let replacement = result?;
            if super::is_callable_value(&replacement) {
                class_val = replacement;
            } else if !matches!(replacement, Value::Undefined) {
                return Err(VmErr::Msg(
                    "TypeError: class decorator must return a callable or undefined".into(),
                ));
            }
        }

        if !name.is_empty() && private_scope.borrow().own_binding(&name).is_some() {
            private_scope
                .borrow_mut()
                .declare(&name, class_val.clone(), BindKind::Const, true);
        }
        if let Some(initializers) = static_method_initializers {
            self.run_method_initializers(&initializers, &class_val)?;
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
            scope.borrow_mut().set(HOME_OBJECT, original_class.clone());
            let saved = std::mem::replace(&mut self.global, scope);
            let result = (|| {
                self.execution.check()?;
                match element {
                    ClassStaticElement::Block(block) => self.run_program_body(&block).map(|_| ()),
                    ClassStaticElement::Field {
                        decoration,
                        name,
                        private,
                        init,
                    } => {
                        let value = if private {
                            self.evaluate_private_class_field(&init, &name)?
                        } else {
                            self.evaluate_class_field(&init, &name)?
                        };
                        let value = if let Some(state) = &decoration {
                            self.apply_field_initializers(state, &class_val, value)?
                        } else {
                            value
                        };
                        let stored = if private {
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
                        };
                        stored?;
                        if let Some(state) = &decoration {
                            self.run_field_extra_initializers(state, &class_val)?;
                        }
                        Ok(())
                    }
                }
            })();
            self.global = saved;
            result?;
        }
        if let Some(extra_initializers) = extra_initializers {
            let initializers = extra_initializers
                .as_array()
                .expect("internal initializer list");
            for index in 0..initializers.array_length() {
                self.check_execution_interrupt()?;
                let initializer = initializers
                    .index_value(index)
                    .expect("dense initializer list");
                self.call_this(&initializer, class_val.clone(), Vec::new())?;
            }
        }
        Ok(class_val)
    }

    fn build_class(
        &mut self,
        name: &str,
        superclass: Option<&Expr>,
        body: &[ClassMember],
        decorators: &[Expr],
        source_text: Option<&crate::parser::SourceTextRef>,
    ) -> Result<Value, VmErr> {
        self.build_class_with_name(
            name,
            crate::parser::declaration_display_name(name),
            superclass,
            body,
            decorators,
            source_text,
        )
    }

    fn build_class_with_name(
        &mut self,
        binding_name: &str,
        name: &str,
        superclass: Option<&Expr>,
        body: &[ClassMember],
        decorators: &[Expr],
        source_text: Option<&crate::parser::SourceTextRef>,
    ) -> Result<Value, VmErr> {
        let mut evaluated_decorators = Vec::new();
        for decorator in decorators {
            evaluated_decorators.push(
                self.evaluate_call_reference(decorator)?
                    .unwrap_or((Value::Undefined, Value::Undefined)),
            );
        }
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
        let has_method_decorators = body.iter().any(|member| !member.decorators().is_empty());
        let mut method_definitions = Vec::new();
        let mut field_definitions = Vec::new();

        for (member_index, member) in body.iter().enumerate() {
            let saved = std::mem::replace(&mut self.global, member_scope.clone());
            let member_decorators = member
                .decorators()
                .iter()
                .map(|decorator| {
                    self.evaluate_call_reference(decorator)
                        .map(|reference| reference.unwrap_or((Value::Undefined, Value::Undefined)))
                })
                .collect::<Result<Vec<_>, VmErr>>();
            self.global = saved;
            let member_decorators = member_decorators?;
            match member {
                ClassMember::Method {
                    decorators: _,
                    source_text: _,
                    name,
                    is_static: st,
                    params: mp,
                    body: mb,
                    is_async,
                    is_generator,
                } => {
                    let key = self.class_member_name(&member_scope, name)?;
                    if *st && !matches!(name, MemberName::Private(_)) {
                        Self::check_static_method_key(&key)?;
                    }
                    let mname = self.property_key(&key)?;
                    let display_name = self.property_function_name(&key)?;
                    if !matches!(name, MemberName::Private(_)) {
                        if *st {
                            static_keys.push(key.clone());
                        } else {
                            proto_keys.push(key.clone());
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

                    let fn_val = fn_val.with_function_source(member.source_text());
                    if has_method_decorators && (*st || !is_ctor_name) {
                        method_definitions.push(super::ClassMethodDefinition {
                            auto_decorator_field: None,
                            kind: super::ClassMethodKind::Method,
                            key: key.clone(),
                            private: matches!(name, MemberName::Private(_)),
                            is_static: *st,
                            value: fn_val.clone(),
                            decorators: member_decorators,
                        });
                    }

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
                    decorators: _,
                    name,
                    is_static: st,
                    auto_accessor,
                    init,
                } => {
                    let fname = self.class_member_name(&member_scope, name)?;
                    let auto_decorator_field = (!member_decorators.is_empty() && *auto_accessor)
                        .then_some(field_definitions.len());
                    if !member_decorators.is_empty() {
                        field_definitions.push(super::ClassFieldDefinition {
                            state: None,
                            key: fname.clone(),
                            private: matches!(name, MemberName::Private(_)),
                            is_static: *st,
                            index: if *st {
                                static_elements.len()
                            } else {
                                instance_fields.len()
                            },
                            decorators: member_decorators,
                        });
                    }

                    if *auto_accessor {
                        let storage = crate::parser::auto_accessor_storage(member_index);
                        member_scope.borrow_mut().declare(
                            &storage,
                            fname.clone(),
                            BindKind::Const,
                            true,
                        );
                        let key = self.property_key(&fname)?;
                        let display_name = self.property_function_name(&fname)?;
                        let mut accessors = Vec::new();
                        for setter in [false, true] {
                            let body = crate::parser::auto_accessor_body(&storage, setter);
                            let function = Value::Function(Rc::new(FunctionData {
                                strict: true,
                                native: None,
                                identity: Rc::new(0),
                                name: Some(
                                    format!(
                                        "{} {display_name}",
                                        if setter { "set" } else { "get" }
                                    )
                                    .into(),
                                ),
                                properties: FunctionData::properties_with_default_prototype(
                                    &self.persistent_global,
                                ),
                                standard_properties_initialized: Rc::new(std::cell::Cell::new(
                                    false,
                                )),
                                params: Rc::new(if setter {
                                    vec![Rc::from("value")]
                                } else {
                                    vec![]
                                }),
                                body: Rc::new(body),
                                closure: Some(crate::heap::capture_env(if *st {
                                    &static_member_closure
                                } else {
                                    &member_closure
                                })),
                                is_arrow: false,
                                is_constructor: false,
                                is_async: false,
                                is_generator: false,
                                uses_arguments: false,
                                bytecode: None,
                                needs_hoisting: false,
                                bound: None,
                            }));
                            accessors.push(function);
                        }
                        let setter = accessors.pop().unwrap();
                        let getter = accessors.pop().unwrap();
                        if has_method_decorators {
                            for (kind, value) in [
                                (super::ClassMethodKind::Getter, getter.clone()),
                                (super::ClassMethodKind::Setter, setter.clone()),
                            ] {
                                method_definitions.push(super::ClassMethodDefinition {
                                    auto_decorator_field: if kind == super::ClassMethodKind::Getter
                                    {
                                        auto_decorator_field
                                    } else {
                                        None
                                    },
                                    kind,
                                    key: fname.clone(),
                                    private: matches!(name, MemberName::Private(_)),
                                    is_static: *st,
                                    value,
                                    decorators: Vec::new(),
                                });
                            }
                        }

                        if matches!(name, MemberName::Private(_)) {
                            define_class_private_element(
                                &member_scope,
                                &mut private_statics,
                                &key,
                                *st,
                                PrivateElement::Accessor {
                                    get: Some(getter),
                                    set: Some(setter),
                                },
                            )?;
                        } else if *st {
                            Self::check_static_method_key(&fname)?;
                            static_keys.push(fname.clone());
                            insert_class_accessor(&mut statics, &key, getter);
                            insert_class_accessor(&mut statics, &key, setter);
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
                            proto_keys.push(fname.clone());
                            insert_class_accessor(&mut proto_props, &key, getter);
                            insert_class_accessor(&mut proto_props, &key, setter);
                        }
                        if *st {
                            static_elements.push(ClassStaticElement::Field {
                                decoration: None,
                                name: Value::String(crate::JsString::from_key(&storage)),
                                private: true,
                                init: init.clone().unwrap_or(Expr::Undefined),
                            });
                        } else {
                            instance_fields.push((
                                Expr::String(crate::JsString::from_key(&storage)),
                                init.clone(),
                                false,
                            ));
                        }
                        continue;
                    }

                    if *st {
                        static_elements.push(ClassStaticElement::Field {
                            decoration: None,
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
                    decorators: _,
                    source_text: _,
                    name,
                    is_static: st,
                    body: gb,
                } => {
                    let key = self.class_member_name(&member_scope, name)?;
                    if *st && !matches!(name, MemberName::Private(_)) {
                        Self::check_static_method_key(&key)?;
                    }
                    let gname = self.property_key(&key)?;
                    let display_name = self.property_function_name(&key)?;
                    if !matches!(name, MemberName::Private(_)) {
                        if *st {
                            static_keys.push(key.clone());
                        } else {
                            proto_keys.push(key.clone());
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

                    let getter_fn = getter_fn.with_function_source(member.source_text());
                    if has_method_decorators {
                        method_definitions.push(super::ClassMethodDefinition {
                            auto_decorator_field: None,
                            kind: super::ClassMethodKind::Getter,
                            key: key.clone(),
                            private: matches!(name, MemberName::Private(_)),
                            is_static: *st,
                            value: getter_fn.clone(),
                            decorators: member_decorators,
                        });
                    }

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
                    decorators: _,
                    source_text: _,
                    name,
                    param,
                    is_static: st,
                    body: sb,
                } => {
                    let key = self.class_member_name(&member_scope, name)?;
                    if *st && !matches!(name, MemberName::Private(_)) {
                        Self::check_static_method_key(&key)?;
                    }
                    let sname = self.property_key(&key)?;
                    let display_name = self.property_function_name(&key)?;
                    if !matches!(name, MemberName::Private(_)) {
                        if *st {
                            static_keys.push(key.clone());
                        } else {
                            proto_keys.push(key.clone());
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

                    let setter_fn = setter_fn.with_function_source(member.source_text());
                    if has_method_decorators {
                        method_definitions.push(super::ClassMethodDefinition {
                            auto_decorator_field: None,
                            kind: super::ClassMethodKind::Setter,
                            key: key.clone(),
                            private: matches!(name, MemberName::Private(_)),
                            is_static: *st,
                            value: setter_fn.clone(),
                            decorators: member_decorators,
                        });
                    }

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
        if has_method_decorators
            || super_cls.is_some()
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
            field_definitions,
            method_definitions,
            decorators: evaluated_decorators,
            source_text: source_text.cloned(),
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
        module: &crate::JsString,
        phase: crate::parser::ImportPhase,
        default: Option<&str>,
        named: &[(String, String)],
        namespace: Option<&str>,
        attributes: &[(String, crate::JsString)],
    ) -> Result<Value, VmErr> {
        // Linking has already installed import bindings for an ESM graph.
        // Executing the declaration itself must not make a deferred edge eager.
        if self.is_linked_module() {
            return Ok(Value::Undefined);
        }
        let module = Self::host_module_specifier(module)?;
        if phase == crate::parser::ImportPhase::Source {
            let value = self.load_source_module_object(&module, attributes)?;
            self.declare_binding(
                default.expect("source binding"),
                value,
                BindKind::Const,
                true,
            )?;
            return Ok(Value::Undefined);
        }
        let resolved_module = if attributes.is_empty() {
            self.resolve_module_request(&module)?
        } else {
            Some(self.load_module_with_attributes(&module, attributes)?)
        };
        if let Some(name) = resolved_module.as_ref() {
            let name = name.clone();
            if phase == crate::parser::ImportPhase::Deferred {
                self.link_module(&name)?;
            } else {
                self.ensure_module(&name)?;
            }
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
                let namespace_object = if phase == crate::parser::ImportPhase::Deferred {
                    Self::deferred_namespace_object(
                        &md,
                        resolved_module
                            .as_deref()
                            .expect("resolved deferred import"),
                    )?
                } else {
                    Self::namespace_object(&md)?
                };
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
        source: Option<&crate::JsString>,
    ) -> Result<Value, VmErr> {
        if self.is_linked_module() {
            return Ok(Value::Undefined);
        }
        match source {
            // `export { a, b as c } from 'm'`: forward the *other*
            // module's live bindings without binding anything locally.
            Some(source) => {
                let source = Self::host_module_specifier(source)?;
                let entries = self.resolve_reexports(&source, specifiers)?;
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
        source: &crate::JsString,
        alias: Option<&str>,
    ) -> Result<Value, VmErr> {
        if self.is_linked_module() {
            return Ok(Value::Undefined);
        }
        let source = Self::host_module_specifier(source)?;
        let resolved = self
            .resolve_module_request(&source)?
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
        self.eval_dynamic_import_with_options(
            specifier,
            None,
            crate::parser::ImportPhase::Evaluation,
        )
    }

    fn eval_dynamic_import_with_options(
        &mut self,
        specifier: Value,
        options: Option<Value>,
        phase: crate::parser::ImportPhase,
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
                    let mut converted = Vec::with_capacity(keys.len());
                    for key in keys {
                        let value = self.get_prop_value_str(&attributes, &key)?;
                        let Value::String(value) = &value else {
                            return Err(VmErr::Msg(
                                "TypeError: import attribute values must be strings".into(),
                            ));
                        };
                        converted.push((key, value.clone()));
                    }
                    return Ok((name, converted));
                }
            }
            Ok((name, Vec::new()))
        })();
        match converted {
            Ok((specifier, attributes)) => {
                self.jobs
                    .borrow_mut()
                    .push_microtask(super::jobs::Job::DynamicImport {
                        realm: self.persistent_global.clone(),
                        target: target.clone(),
                        specifier,
                        attributes,
                        phase,
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

    /// Update the host main flag while preserving a previously exposed meta object.
    pub fn set_import_meta_main(&mut self, is_main: bool) -> Result<(), VmErr> {
        let cached = self.current_module().import_meta.clone();
        if let Some(meta) = cached {
            meta.set_prop("main".into(), Value::Bool(is_main))?;
        }
        self.is_main = is_main;
        Ok(())
    }

    /// Shared `import.meta`: the current module's URL and main flag.
    pub(crate) fn eval_import_meta(&mut self) -> Result<Value, VmErr> {
        let cached = self.current_module().import_meta.clone();
        if let Some(value) = cached {
            return Ok(value);
        }
        let url = self
            .cur_mod
            .as_ref()
            .and_then(|module| self.module_file_urls.borrow().get(module).cloned())
            .unwrap_or_else(|| "vm://module".into());
        let o = vec![
            ("url".to_string(), Value::String((url).into())),
            ("main".to_string(), Value::Bool(self.is_main)),
        ];
        let meta = Value::object_with_proto(o, None);
        self.current_module().import_meta = Some(meta.clone());
        Ok(meta)
    }

    pub(crate) fn instantiate_function_declaration(
        &mut self,
        statement: &Statement,
    ) -> Result<Value, VmErr> {
        let Statement::FnDecl {
            name,
            params,
            body,
            is_async,
            is_generator,
            ..
        } = statement
        else {
            unreachable!()
        };

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
                name: Some(crate::parser::declaration_display_name(name).into()),
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
            }))
            .with_function_source(statement.source_text()),
        )?;
        Ok(Value::Undefined)
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
                // `var` was already hoisted to the enclosing function or
                // program scope; this statement only assigns to it.
                // ResolveBinding precedes Initializer evaluation, so an
                // initializer that deletes the with-object property the
                // declaration writes through still writes to that object.
                if matches!(kind, VarKind::Var) && destructuring.is_none() {
                    if let Some(e) = init {
                        let (base, property) =
                            self.resolve_assignment_binding(&self.global.clone(), name)?;
                        let v =
                            self.evaluate_class_field(e, &Value::String(name.as_str().into()))?;
                        self.write_assignment_binding(&base, property, name, v.clone())?;
                        return Ok(v);
                    }
                    // A bare `var a;` re-declaration must not erase an
                    // existing value: `var a = 1; var a; a` is 1.
                    // Hoisting already created the binding.
                    return Ok(Value::Undefined);
                }
                let v = match init {
                    Some(e) if destructuring.is_none() => {
                        self.evaluate_class_field(e, &Value::String(name.as_str().into()))?
                    }
                    Some(e) => self.eval_expr(e)?,
                    None => Value::Undefined,
                };
                match kind {
                    VarKind::Var => {
                        if let Some(pat) = destructuring {
                            // Hoisted var bindings use resolved references,
                            // captured before source reads and defaults.
                            self.destructure_assignment(pat, &v)?;
                        } else if init.is_some() {
                            self.assign_or_set_binding(name, v.clone())?;
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
            Statement::FnDecl { .. } => self.evaluate_function_declaration(s),
            Statement::ClassDecl {
                decorators,
                source_text: _,
                name,
                superclass,
                body,
            } => {
                let class_val = self.build_class(
                    name,
                    superclass.as_deref(),
                    body,
                    decorators,
                    s.source_text(),
                )?;
                self.set_binding(name, class_val)?;
                Ok(Value::Undefined)
            }
            Statement::Return(e) => {
                let v = match e {
                    Some(ex) => {
                        let value =
                            self.eval_expr_in_tail_position(ex, self.tail_call_is_allowed())?;
                        self.prepare_return_value(value)?
                    }
                    None => Value::Undefined,
                };
                vm_ret(v)
            }
            Statement::If { test, then, else_ } => {
                let t = self.eval_expr(test)?;
                let result = if self.truthy(&t) {
                    self.run_block(then)
                } else if let Some(a) = else_ {
                    self.run_block(a)
                } else {
                    Ok(Value::Undefined)
                };
                result.map_err(|error| error.update_empty(Some(Value::Undefined)))
            }
            Statement::ResourceForOf {
                name,
                iter,
                body,
                is_await,
                await_disposal,
            } => {
                let binding = ForBinding::Declaration {
                    kind: VarKind::Const,
                    pattern: Pattern::Ident(name.clone()),
                    initializer: None,
                };
                self.with_loop_binding_scope(&binding, |vm| {
                    vm.run_for_of(&binding, iter, body, *is_await, Some(*await_disposal))
                })
            }
            Statement::ResourceDeclaration {
                declarations,
                is_await,
            } => {
                for declaration in declarations {
                    let Statement::VarDecl {
                        name,
                        init: Some(init),
                        ..
                    } = declaration
                    else {
                        return vm_err("SyntaxError: invalid resource declaration");
                    };
                    let value =
                        self.evaluate_class_field(init, &Value::String(name.as_str().into()))?;
                    self.acquire_resource(value.clone(), *is_await)?;
                    self.declare_binding(name, value, BindKind::Const, true)?;
                }
                Ok(Value::Undefined)
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
                result.map_err(|error| error.update_empty(Some(Value::Undefined)))
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
                    match self.run_block_with_lexical_scope_completion(body, body_needs_scope) {
                        Err(VmErr::Break(l, value)) if l.is_none() || label_matches(&label, &l) => {
                            if let Some(value) = value {
                                r = *value;
                            }
                            break;
                        }
                        Err(VmErr::Continue(l, value))
                            if l.is_none() || label_matches(&label, &l) =>
                        {
                            if let Some(value) = value {
                                r = *value;
                            }
                            continue;
                        }
                        Ok(Some(value)) => r = value,
                        Ok(None) => {}
                        Err(error) => return Err(error.update_empty(Some(r))),
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
                    match self.run_block_with_lexical_scope_completion(body, body_needs_scope) {
                        Err(VmErr::Break(l, value)) if l.is_none() || label_matches(&label, &l) => {
                            if let Some(value) = value {
                                r = *value;
                            }
                            break;
                        }
                        Err(VmErr::Continue(l, value))
                            if l.is_none() || label_matches(&label, &l) =>
                        {
                            if let Some(value) = value {
                                r = *value;
                            }
                        }
                        Ok(Some(value)) => r = value,
                        Ok(None) => {}
                        Err(error) => return Err(error.update_empty(Some(r))),
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
                vm.run_for_of(binding, iter, body, *is_await, None)
            }),
            Statement::Block(s) => self.run_block(s),
            // A declarator group shares the enclosing scope: no new frame.
            Statement::Declarations(s) => self.run(s),
            Statement::ParameterInitialization { .. } | Statement::ClassInitialization { .. } => {
                Err(VmErr::Msg(
                    "Invalid parameter initialization position".into(),
                ))
            }
            Statement::Labeled { .. } => self
                .eval_stmt_completion(s)
                .map(|value| value.unwrap_or(Value::Undefined)),
            Statement::Break => Err(VmErr::Break(None, None)),
            Statement::Continue => Err(VmErr::Continue(None, None)),
            Statement::LabeledBreak(label) => Err(VmErr::Break(Some(label.clone()), None)),
            Statement::LabeledContinue(label) => Err(VmErr::Continue(Some(label.clone()), None)),
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
                let saved_barriers = self.tail_call_barriers;
                if catch.is_some() || finally.is_some() {
                    self.tail_call_barriers += 1;
                }
                let body_result = self.run_block(body);
                self.tail_call_barriers = saved_barriers;
                if let Err(error) = &body_result
                    && error.is_abandon()
                {
                    return body_result;
                }

                if finally.is_some() {
                    self.tail_call_barriers += 1;
                }
                let after_catch = match body_result {
                    Err(VmErr::Throw(val)) => self.run_catch(catch, val),
                    // Control-flow signals are not catchable.
                    Err(e @ (VmErr::Break(..) | VmErr::Continue(..))) => Err(e),
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

                self.tail_call_barriers = saved_barriers;
                // finally always runs last; its own error/return takes precedence.
                if let Some(f) = finally {
                    self.run_block(f)
                        .map_err(|error| error.update_empty(Some(Value::Undefined)))?;
                }
                after_catch.map_err(|error| error.update_empty(Some(Value::Undefined)))
            }
            Statement::Switch { disc, cases } => {
                let d = self.eval_expr(disc)?;
                // Every case shares one block scope: fall-through means a
                // `let` declared in one case is visible in the next.
                let outer = self.push_scope();
                let result = self.run_switch_cases(&d, cases);
                let result = self.finish_resources(&self.global.clone(), result);
                self.pop_scope(outer);
                result
            }
            Statement::ExportDefault(e) => {
                let v = self.evaluate_class_field(e, &Value::String("default".into()))?;
                self.stmt_export_default(v)
            }
            Statement::ExportNamed {
                specifiers, source, ..
            } => self.stmt_export_named(specifiers, source.as_ref()),
            Statement::ExportAll { source, alias, .. } => {
                self.stmt_export_all(source, alias.as_deref())
            }
            Statement::Import {
                module,
                phase,
                default,
                named,
                namespace,
                attributes,
            } => self.stmt_import(
                module,
                *phase,
                default.as_deref(),
                named,
                namespace.as_deref(),
                attributes,
            ),
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
        self.drain_iterator_with_next(iterator, next_fn)
    }

    pub(crate) fn drain_iterator_with_next(
        &mut self,
        iterator: &Value,
        next_fn: Value,
    ) -> Result<Vec<Value>, VmErr> {
        // A dense Array iterator with the intrinsic next method cannot invoke
        // guest code while reading its elements. Avoid allocating a temporary
        // result object and entering a callable realm for every element. Keep
        // cursor updates and the final done step, including their loop budget.
        let native = match &next_fn {
            Value::Function(function) if function.bound.is_none() => function.native,
            Value::NativeFunction { callable, .. } => Some(*callable),
            _ => None,
        };
        if native.is_some_and(|native| {
            std::ptr::fn_addr_eq(
                native,
                super::resolve::array_iter_next as crate::builtins::NativeFn,
            )
        }) && let Value::Object { props } = iterator
            && !props.meta.borrow().has_accessors
            && let Some(Value::Array(array)) = props.own_value("__items__").as_ref()
            && !array.meta.borrow().has_accessors
            && array.array_length() <= crate::value::MAX_ARRAY_LEN
            && array.array_length() == array.borrow().len()
            && array.presence_snapshot().iter().all(|present| *present)
            && matches!(props.own_value("__kind__"), Some(Value::String(ref kind)) if kind == "values")
        {
            let mut cursor = match props.own_value("__cursor__") {
                Some(Value::Number(cursor)) => cursor as usize,
                _ => 0,
            };
            let length = array.array_length();
            let mut out = Vec::with_capacity(length.saturating_sub(cursor));
            loop {
                self.consume_loop()?;
                if cursor >= length {
                    iterator.set_prop("__items__".into(), Value::Undefined)?;
                    return Ok(out);
                }
                iterator.set_prop("__cursor__".into(), Value::Number((cursor + 1) as f64))?;
                out.push(
                    array
                        .index_value(cursor)
                        .unwrap_or(Value::Undefined)
                        .deref_binding(),
                );
                cursor += 1;
            }
        }
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
            let lexical = match init {
                ForInit::Var {
                    kind: kind @ (VarKind::Let | VarKind::Const),
                    decls,
                } => Some((
                    kind.clone(),
                    decls
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>(),
                )),
                ForInit::Pattern {
                    kind: kind @ (VarKind::Let | VarKind::Const),
                    pattern,
                    trailing,
                    ..
                } => {
                    let mut names = crate::parser::pattern_names(pattern);
                    names.extend(trailing.iter().map(|(name, _)| name.clone()));
                    Some((kind.clone(), names))
                }
                _ => None,
            };
            if let Some((kind, names)) = lexical {
                let binding_kind = if kind == VarKind::Const {
                    BindKind::Const
                } else {
                    BindKind::Let
                };
                for name in &names {
                    self.declare_binding(name, Value::Undefined, binding_kind, false)?;
                }
                if kind == VarKind::Let {
                    per_iteration = names;
                }
            }
            match init {
                ForInit::Var { kind, decls } => {
                    for (name, initializer) in decls {
                        if *kind == VarKind::Var && initializer.is_none() {
                            continue;
                        }
                        let value = match initializer {
                            Some(expression) => self.evaluate_class_field(
                                expression,
                                &Value::String(name.as_str().into()),
                            )?,
                            None => Value::Undefined,
                        };
                        if *kind == VarKind::Var {
                            self.assign_or_set_binding(name, value)?;
                        } else {
                            self.global.borrow_mut().initialize(name, value);
                        }
                    }
                }
                ForInit::Pattern {
                    kind,
                    pattern,
                    init,
                    trailing,
                } => {
                    let value = self.eval_expr(init)?;
                    if *kind == VarKind::Var {
                        self.destructure_assignment(pattern, &value)?;
                    } else {
                        self.destructure(pattern, &value)?;
                    }
                    for (name, initializer) in trailing {
                        if *kind == VarKind::Var && initializer.is_none() {
                            continue;
                        }
                        let value = match initializer {
                            Some(expression) => self.evaluate_class_field(
                                expression,
                                &Value::String(name.as_str().into()),
                            )?,
                            None => Value::Undefined,
                        };
                        if *kind == VarKind::Var {
                            self.assign_or_set_binding(name, value)?;
                        } else {
                            self.global.borrow_mut().initialize(name, value);
                        }
                    }
                }
                ForInit::Expr(expression) => {
                    self.eval_expr(expression)?;
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
            match self.run_block_with_lexical_scope_completion(body, body_needs_scope) {
                Err(VmErr::Break(l, value)) if l.is_none() || label_matches(&label, &l) => {
                    if let Some(value) = value {
                        r = *value;
                    }
                    break;
                }
                Err(VmErr::Continue(l, value)) if l.is_none() || label_matches(&label, &l) => {
                    if let Some(value) = value {
                        r = *value;
                    }
                }
                Ok(Some(value)) => r = value,
                Ok(None) => {}
                Err(error) => return Err(error.update_empty(Some(r))),
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
        match callee {
            Expr::Member {
                object,
                property,
                computed,
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    let key = self.evaluate_super_property_key(property)?;
                    let (base, receiver, key) = self.super_reference(&self.global.clone(), &key)?;
                    let function = self.get_prop_value_with_receiver(&base, &key, &receiver)?;
                    return Ok(Some((function, receiver)));
                }
                let Some(receiver) = self.evaluate_chain_value(object)? else {
                    return Ok(None);
                };
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
                object,
                property,
                computed,
            } => {
                if !computed && matches!(property.as_ref(), Expr::Undefined) {
                    let Some((function, receiver)) = self.evaluate_call_reference(object)? else {
                        return Ok(None);
                    };
                    return Ok((!matches!(function, Value::Null | Value::Undefined))
                        .then_some((function, receiver)));
                }
                let Some(receiver) = self.evaluate_chain_value(object)? else {
                    return Ok(None);
                };
                if matches!(receiver, Value::Null | Value::Undefined) {
                    return Ok(None);
                }
                let key = self.eval_expr(property)?;
                let function = if !computed
                    && matches!(&key, Value::String(name) if name.to_key().starts_with('#'))
                {
                    self.get_private_member(&receiver, &self.property_key(&key)?)?
                } else {
                    self.get_prop_value(&receiver, &key)?
                };
                Ok(Some((function, receiver)))
            }
            Expr::Parenthesized(inner) => Ok(Some(
                self.evaluate_call_reference(inner)?
                    .unwrap_or((Value::Undefined, Value::Undefined)),
            )),
            Expr::Call { callee, args } => Ok(self
                .evaluate_chain_call(callee, args, false)?
                .map(|value| (value, Value::Undefined))),
            Expr::Identifier(name) => self.read_call_binding(&self.global.clone(), name).map(Some),
            other => Ok(Some((self.eval_expr(other)?, Value::Undefined))),
        }
    }

    fn evaluate_chain_value(&mut self, expression: &Expr) -> Result<Option<Value>, VmErr> {
        if matches!(
            expression,
            Expr::Member { .. } | Expr::OptionalChain { .. } | Expr::Call { .. }
        ) {
            self.consume_fuel(1)?;
        }
        match expression {
            Expr::Member { .. } | Expr::OptionalChain { .. } => Ok(self
                .evaluate_call_reference(expression)?
                .map(|(value, _)| value)),
            Expr::Call { callee, args } => self.evaluate_chain_call(callee, args, false),
            _ => self.eval_expr(expression).map(Some),
        }
    }

    fn evaluate_chain_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        tail: bool,
    ) -> Result<Option<Value>, VmErr> {
        if matches!(callee.unparenthesized(), Expr::Super) {
            let target = self.super_constructor(&self.global.clone())?;
            let args = self.evaluate_call_arguments(args)?;
            return self.invoke_ctor(&target, Value::Undefined, args).map(Some);
        }
        let direct_eval =
            matches!(callee.unparenthesized(), Expr::Identifier(name) if name == "eval");
        let Some((function, receiver)) = self.evaluate_call_reference(callee)? else {
            return Ok(None);
        };
        let arguments = self.evaluate_call_arguments(args)?;
        let result = if direct_eval
            && crate::builtins::is_intrinsic_eval(&function, &self.persistent_global)
        {
            crate::builtins::eval_direct(self, arguments)
        } else if tail {
            self.request_tail_call(function, receiver, arguments)
        } else {
            self.call_this(&function, receiver, arguments)
        };
        result.map(Some)
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
            let value = if let Pattern::Ident(name) = pattern {
                self.evaluate_class_field(value, &Value::String(name.as_str().into()))?
            } else {
                self.eval_expr(value)?
            };
            self.destructure(pattern, &value)?;
        }
        let o = self.eval_expr(obj)?;
        let mut enumerator = super::for_in::ForInEnumerator::new(&o)?;
        let body_needs_scope = block_needs_lexical_scope(body);
        let mut r = Value::Undefined;
        let label = self.active_label.take();
        while let Some(k) = enumerator.next(self)? {
            self.consume_loop()?;
            self.enter_iteration_binding_scope(binding);
            self.assign_iteration_binding(binding, &Value::String(k.into()))?;
            match self.run_block_with_lexical_scope_completion(body, body_needs_scope) {
                Err(VmErr::Break(l, value)) if l.is_none() || label_matches(&label, &l) => {
                    if let Some(value) = value {
                        r = *value;
                    }
                    break;
                }
                Err(VmErr::Continue(l, value)) if l.is_none() || label_matches(&label, &l) => {
                    if let Some(value) = value {
                        r = *value;
                    }
                    continue;
                }
                Ok(Some(value)) => r = value,
                Ok(None) => {}
                Err(error) => return Err(error.update_empty(Some(r))),
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
        disposal: Option<bool>,
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
            let assignment = (|| {
                if let Some(asynchronous) = disposal {
                    self.acquire_resource(value.clone(), asynchronous)?;
                }
                self.assign_iteration_binding(binding, &value)
            })();
            if let Err(error) = assignment {
                if !error.is_abandon() {
                    let _ = self.close_guest_iterator(&iterator, is_await);
                }
                return Err(error);
            }
            self.tail_call_barriers += 1;
            let outcome = self.run_block_with_lexical_scope_completion(body, body_needs_scope);
            self.tail_call_barriers -= 1;
            let outcome = if disposal.is_some() {
                self.finish_completion_resources(&self.global.clone(), outcome)
            } else {
                outcome
            };
            match outcome {
                Err(VmErr::Break(l, value)) if l.is_none() || label_matches(&label, &l) => {
                    if let Some(value) = value {
                        r = *value;
                    }
                    break;
                }
                Err(VmErr::Continue(l, value)) if l.is_none() || label_matches(&label, &l) => {
                    if let Some(value) = value {
                        r = *value;
                    }
                    continue;
                }
                // `return`, `throw`, or a break/continue aimed at an
                // outer label also leaves the loop, and also closes.
                // An abandon teardown is the exception: it runs no
                // handlers, so it must not close either.
                Err(error) => {
                    let error = error.update_empty(Some(r));
                    if !error.is_abandon() {
                        self.close_guest_iterator_for_abrupt(&iterator, is_await, &error)?;
                    }
                    return Err(error);
                }
                Ok(Some(value)) => r = value,
                Ok(None) => {}
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
                    let value = self.eval_expr(&Expr::Identifier(name.clone()))?;
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
                    // NamedEvaluation: an anonymous function or class value
                    // takes the property key as its name, shared with class
                    // field initializers.
                    let key_value = Value::String(crate::JsString::from_key(key));
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        key.clone(),
                        self.evaluate_class_field(expression, &key_value)?,
                        None,
                    );
                }
                ObjectProp::ComputedMethod {
                    source_text: _,
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
                    let function = self
                        .object_literal_callable(
                            &home,
                            &function_name,
                            params,
                            body,
                            *is_async,
                            *is_generator,
                        )
                        .with_function_source(prop.source_text());
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
                        self.evaluate_class_field(value_expression, &key_value)?,
                        None,
                    );
                    if let Some(symbol) = symbol {
                        symbol_keys.push((key, symbol));
                    }
                }
                ObjectProp::Method {
                    source_text: _,
                    name,
                    params,
                    body,
                    is_async,
                    is_generator,
                } => {
                    let function = self
                        .object_literal_callable(
                            &home,
                            name,
                            params,
                            body,
                            *is_async,
                            *is_generator,
                        )
                        .with_function_source(prop.source_text());
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        name.clone(),
                        function,
                        None,
                    );
                }
                ObjectProp::Getter {
                    source_text: _,
                    name,
                    body,
                } => {
                    let function = self
                        .object_literal_callable(
                            &home,
                            &format!("get {name}"),
                            &[],
                            body,
                            false,
                            false,
                        )
                        .with_function_source(prop.source_text());
                    insert_object_property(
                        &mut object,
                        &mut positions,
                        &mut accessors,
                        name.clone(),
                        function,
                        Some(ObjectAccessorKind::Getter),
                    );
                }
                ObjectProp::Setter {
                    source_text: _,
                    name,
                    param,
                    body,
                } => {
                    let function = self
                        .object_literal_callable(
                            &home,
                            &format!("set {name}"),
                            std::slice::from_ref(param),
                            body,
                            false,
                            false,
                        )
                        .with_function_source(prop.source_text());
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
                    self.for_each_spread_entry(&value, &[], |key, property_value| {
                        let key = match &key {
                            Value::String(name) => name.to_key(),
                            Value::Symbol(symbol) => {
                                let key = super::symbol_slot_key(symbol);
                                symbol_keys.push((key.clone(), symbol.clone()));
                                key
                            }
                            _ => unreachable!("own property keys are strings or symbols"),
                        };
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

        // Select a clause before executing its body. A default clause is only
        // used if no selector matches; selectors after a match are not evaluated.
        let mut selected = None;
        let mut default = None;
        for (index, case) in cases.iter().enumerate() {
            if let Some(test) = &case.test {
                let value = self.eval_expr(test)?;
                if self.seq(disc, &value) {
                    selected = Some(index);
                    break;
                }
            } else {
                default = Some(index);
            }
        }
        let mut value = Value::Undefined;
        if let Some(start) = selected.or(default) {
            for case in &cases[start..] {
                match self.run_completion(&case.body) {
                    Ok(Some(next)) => value = next,
                    Ok(None) => {}
                    Err(error) => {
                        let error = error.update_empty(Some(value));
                        return match error {
                            VmErr::Break(None, value) => {
                                Ok(*value.expect("switch completion value"))
                            }
                            other => Err(other),
                        };
                    }
                }
            }
        }
        Ok(value)
    }

    pub(crate) fn eval_expr(&mut self, e: &Expr) -> Result<Value, VmErr> {
        self.eval_expr_in_tail_position(e, false)
    }

    fn eval_expr_in_tail_position(&mut self, e: &Expr, tail: bool) -> Result<Value, VmErr> {
        self.consume_fuel(1)?;
        match e {
            Expr::LegacyLiteral(inner) | Expr::Parenthesized(inner) => {
                self.eval_expr_in_tail_position(inner, tail)
            }
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
            Expr::Undefined | Expr::Elision => Ok(Value::Undefined),
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
                let mut present = Vec::new();
                for x in i {
                    match x {
                        Expr::Elision => {
                            v.push(Value::Undefined);
                            present.push(false);
                        }
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
                    present.resize(v.len(), true);
                    if v.len() > crate::value::MAX_ARRAY_LEN {
                        return Err(crate::value::limit_err("Maximum array length exceeded"));
                    }
                }
                Value::checked_array_with_presence(v, present)
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
                    | crate::parser::BinOp::Nullish => {
                        return self.eval_expr_in_tail_position(right, tail);
                    }
                    crate::parser::BinOp::Comma => {
                        return self.eval_expr_in_tail_position(right, tail);
                    }
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
                        // `delete super[key]`: `this` resolution throws for
                        // an unbound receiver, then the key expression
                        // evaluates, then deletion throws before any
                        // ToPropertyKey conversion of the key.
                        self.resolve_this(&self.global.clone())?;
                        self.eval_expr(property)?;
                        return Err(VmErr::Msg(
                            "ReferenceError: Cannot delete a super property".into(),
                        ));
                    }
                    match operand.unparenthesized() {
                        Expr::Member {
                            object, property, ..
                        } => {
                            let Some(obj) = self.evaluate_chain_value(object)? else {
                                return Ok(Value::Bool(true));
                            };
                            let key = self.eval_expr(property)?;
                            let strict = self.global.borrow().strict();
                            return self.delete_member_or_throw(&obj, &key, strict);
                        }
                        Expr::OptionalChain {
                            object, property, ..
                        } => {
                            let Some(obj) = self.evaluate_chain_value(object)? else {
                                return Ok(Value::Bool(true));
                            };
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
                        let key = self.evaluate_super_property_key(property)?;
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
                            let Some(obj) = self.evaluate_chain_value(object)? else {
                                return Ok(Value::Bool(true));
                            };
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
                            let strict = self.global.borrow().strict();
                            self.inc_prop_value(&obj, &prop, *op == UnOp::Inc, *prefix, strict)
                        }
                        _ => {
                            let v = self.eval_expr(operand)?;
                            self.un_op(*op, &v)
                        }
                    }
                } else if *op == UnOp::Typeof {
                    // `typeof` tolerates missing identifiers but preserves TDZ errors.
                    let v = if let Expr::Identifier(name) = operand.unparenthesized() {
                        let scope = self.global.clone();
                        match self.lookup_binding_in(&scope, name)? {
                            Lookup::Value(value) => value,
                            Lookup::Missing => Value::Undefined,
                            Lookup::Uninitialized => {
                                return vm_err(format!(
                                    "ReferenceError: Cannot access {name} before initialization"
                                ));
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
            Expr::Call { callee, args } => Ok(self
                .evaluate_chain_call(callee, args, tail)?
                .unwrap_or(Value::Undefined)),
            Expr::Member { .. } | Expr::OptionalChain { .. } => Ok(self
                .evaluate_call_reference(e)?
                .map(|(value, _)| value)
                .unwrap_or(Value::Undefined)),
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
                    let key = self.evaluate_super_property_key(property)?;
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

                if let Expr::Identifier(name) = target.unparenthesized() {
                    let (base, property, current) =
                        self.read_assignment_binding(&self.global.clone(), name)?;
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
                    let assigned = if matches!(target.as_ref(), Expr::Identifier(_)) {
                        self.evaluate_class_field(value, &Value::String(name.into()))?
                    } else {
                        self.eval_expr(value)?
                    };
                    self.write_assignment_binding(&base, property, name, assigned.clone())?;
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
                        let raw_key = self.eval_expr(property)?;
                        if matches!(receiver, Value::Null | Value::Undefined) {
                            return vm_err(
                                "TypeError: Cannot assign a property of null or undefined",
                            );
                        }
                        let key = self.ecmascript_to_property_key(&raw_key)?;
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
                    let key = self.evaluate_super_property_key(property)?;
                    let (base, receiver, key) = if matches!(op, AssignOp::Assign) {
                        let (base, receiver, _) =
                            self.super_reference(&self.global.clone(), &Value::Undefined)?;
                        (base, receiver, key)
                    } else {
                        self.super_reference(&self.global.clone(), &key)?
                    };
                    let current = op
                        .bin_op()
                        .map(|_| self.get_prop_value_with_receiver(&base, &key, &receiver))
                        .transpose()?;
                    let mut assigned = self.eval_expr(value)?;
                    if let Some(op) = op.bin_op() {
                        assigned =
                            self.apply_binary(op, &current.expect("compound value"), &assigned)?;
                    }
                    let key = if matches!(op, AssignOp::Assign) {
                        self.ecmascript_to_property_key(&key)?
                    } else {
                        key
                    };
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
                if let Some(bin) = op.bin_op() {
                    match target.unparenthesized() {
                        Expr::Identifier(name) => {
                            let scope = self.global.clone();
                            let (base, property, current) =
                                self.read_assignment_binding(&scope, name)?;
                            let rhs = self.eval_expr(value)?;
                            let result = self.apply_binary(bin, &current, &rhs)?;
                            self.write_assignment_binding(&base, property, name, result.clone())?;
                            return Ok(result);
                        }
                        Expr::Member {
                            object,
                            property,
                            computed,
                        } => {
                            let object = self.eval_expr(object)?;
                            let raw_key = self.eval_expr(property)?;
                            if matches!(object, Value::Null | Value::Undefined) {
                                return vm_err(
                                    "TypeError: Cannot assign a property of null or undefined",
                                );
                            }
                            let private = !computed
                                && matches!(property.as_ref(), Expr::String(key) if key.to_key().starts_with('#'));
                            let key = if private {
                                raw_key
                            } else {
                                self.ecmascript_to_property_key(&raw_key)?
                            };
                            let current = if private {
                                self.get_private_member(&object, &self.property_key(&key)?)?
                            } else {
                                self.get_prop_value(&object, &key)?
                            };
                            let rhs = self.eval_expr(value)?;
                            let result = self.apply_binary(bin, &current, &rhs)?;
                            if private {
                                let name = self.property_key(&key)?;
                                self.set_private_member(&object, &name, result.clone())?;
                            } else {
                                self.assign_member(&object, &key, result.clone())?;
                            }
                            return Ok(result);
                        }
                        _ => return vm_err("Invalid compound assignment target"),
                    }
                }
                if matches!(op, AssignOp::Assign) {
                    match target.unparenthesized() {
                        Expr::Identifier(name) => {
                            let (base, property) =
                                self.resolve_assignment_binding(&self.global.clone(), name)?;
                            let assigned = if matches!(target.as_ref(), Expr::Identifier(_)) {
                                self.evaluate_class_field(value, &Value::String(name.into()))?
                            } else {
                                self.eval_expr(value)?
                            };
                            self.write_assignment_binding(&base, property, name, assigned.clone())?;
                            return Ok(assigned);
                        }
                        Expr::Member {
                            object,
                            property,
                            computed,
                        } => {
                            let object = self.eval_expr(object)?;
                            let raw_key = self.eval_expr(property)?;
                            let private = !computed
                                && matches!(property.as_ref(),Expr::String(key) if key.to_key().starts_with('#'));
                            let assigned = self.eval_expr(value)?;
                            let key = raw_key;
                            if private {
                                let name = self.property_key(&key)?;
                                self.set_private_member(&object, &name, assigned.clone())?;
                            } else {
                                self.assign_member(&object, &key, assigned.clone())?;
                            }
                            return Ok(assigned);
                        }
                        _ => (),
                    }
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
                    self.eval_expr_in_tail_position(consequent, tail)
                } else {
                    self.eval_expr_in_tail_position(alternate, tail)
                }
            }
            // A class expression's own name is visible only inside its body,
            // which a child scope provides.
            Expr::ClassExpr { decorators, source_text: _,
                name,
                superclass,
                body,
            } => self.build_class(name.as_deref().unwrap_or(""), superclass.as_deref(), body, decorators, e.source_text()),
            Expr::ArrowFn { source_text: _,
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
            })).with_function_source(e.source_text())),
            Expr::FnExpr { source_text: _,
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
                })).with_function_source(e.source_text());
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
                let specifier = self.eval_expr(specifier)?;
                let options = options
                    .as_deref()
                    .map(|expr| self.eval_expr(expr))
                    .transpose()?;
                self.eval_dynamic_import_with_options(specifier, options, *phase)
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
                site,
                tag,
                cooked,
                raw,
                exprs,
            } => {
                let (tag_fn, this_val) = self
                    .evaluate_call_reference(tag)?
                    .unwrap_or((Value::Undefined, Value::Undefined));
                let strings = if let Some(cached) = self.cached_template_object(*site) {
                    cached
                } else {
                    let cooked = Value::array(
                        cooked
                            .iter()
                            .cloned()
                            .map(|part| part.map(Value::String).unwrap_or(Value::Undefined))
                            .collect(),
                    );
                    let raw = Value::array(raw.iter().cloned().map(Value::String).collect());
                    self.cache_template_object(*site, cooked, raw)
                };
                let mut args = vec![strings];
                for expr in exprs {
                    args.push(self.eval_expr(expr)?);
                }
                if tail {
                    self.request_tail_call(tag_fn, this_val, args)
                } else {
                    self.call_this(&tag_fn, this_val, args)
                }
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
                        let rendered = self.ecmascript_to_string(&val)?;
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
                            let value = if asynchronous {
                                self.perform_await(value)?
                            } else {
                                value
                            };
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
                    if done {
                        let value = self.member(&step, "value")?;
                        return if returning {
                            vm_ret(self.prepare_return_value(value)?)
                        } else {
                            Ok(value)
                        };
                    }
                    #[cfg(stackful_coroutines)]
                    if !asynchronous {
                        received = match self.gen_yielder.as_ref() {
                            Some(yielder) => yielder.suspend_result(step),
                            None => crate::value::GenResume::Next(None),
                        };
                        continue;
                    }
                    let value = self.member(&step, "value")?;
                    // Async delegation preserves values from real async
                    // iterators. The async-from-sync adapter awaits its own
                    // values before producing this result.

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
