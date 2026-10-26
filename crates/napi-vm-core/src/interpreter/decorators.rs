//! Class element decoration shared by both class frontends.
use super::{BindKind, Env, Interpreter};
use crate::error::VmErr;
use crate::value::{PrivateElement, Value};

pub(crate) const INSTANCE_FIELD_INITIALIZERS: &str = "\0decorator-instance-field-initializers";

pub(crate) const INSTANCE_METHOD_INITIALIZERS: &str = "\0decorator-instance-method-initializers";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassMethodKind {
    Method,
    Getter,
    Setter,
}

pub(crate) struct ClassMethodDefinition {
    pub kind: ClassMethodKind,
    pub key: Value,
    pub private: bool,
    pub is_static: bool,
    pub value: Value,
    pub decorators: Vec<(Value, Value)>,
}

pub(crate) struct ClassFieldDefinition {
    pub key: Value,
    pub private: bool,
    pub is_static: bool,
    pub index: usize,
    pub decorators: Vec<(Value, Value)>,
}

fn access_receiver(args: &[Value]) -> Result<Value, VmErr> {
    let receiver = args.first().cloned().unwrap_or(Value::Undefined);
    if !super::call::is_js_object(&receiver) {
        return Err(VmErr::Msg(
            "TypeError: decorator access requires an object".into(),
        ));
    }
    Ok(receiver)
}

fn private_id(state: &Value) -> Option<u64> {
    match state.get_prop("privateId") {
        Some(Value::String(ref id)) => {
            Some(id.to_key().parse().expect("internal private identity"))
        }
        _ => None,
    }
}

fn access_has(vm: &mut Interpreter, state: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let receiver = access_receiver(&args)?;
    let found = if let Some(id) = private_id(&state) {
        receiver.has_private_element(id)
    } else {
        vm.has_property(
            &receiver,
            &state.get_prop("key").expect("internal property key"),
        )?
    };
    Ok(Value::Bool(found))
}

fn access_get(vm: &mut Interpreter, state: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let receiver = access_receiver(&args)?;
    if let Some(id) = private_id(&state) {
        return match receiver.private_element(id)? {
            PrivateElement::Field(value) | PrivateElement::Method(value) => Ok(value),
            PrivateElement::Accessor {
                get: Some(getter), ..
            } => vm.call_this(&getter, receiver, vec![]),
            PrivateElement::Accessor { get: None, .. } => Err(VmErr::Msg(
                "TypeError: private accessor has no getter".into(),
            )),
        };
    }
    vm.get_prop_value(
        &receiver,
        &state.get_prop("key").expect("internal property key"),
    )
}

fn access_set(vm: &mut Interpreter, state: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let receiver = access_receiver(&args)?;
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    if let Some(id) = private_id(&state) {
        match receiver.private_element(id)? {
            PrivateElement::Field(_) => receiver.set_private_field(id, value)?,
            PrivateElement::Accessor {
                set: Some(setter), ..
            } => {
                vm.call_this(&setter, receiver, vec![value])?;
            }
            _ => {
                return Err(VmErr::Msg(
                    "TypeError: private element is not writable".into(),
                ));
            }
        }
    } else {
        let key = state.get_prop("key").expect("internal property key");
        if !vm.set_member_with_receiver(&receiver, &key, value, &receiver)? {
            return Err(VmErr::Msg(
                "TypeError: decorator access cannot set property".into(),
            ));
        }
    }
    Ok(Value::Undefined)
}

impl Interpreter {
    fn element_decorator_context(
        &self,
        scope: &Env,
        kind: &str,
        key: &Value,
        private: bool,
        is_static: bool,
        initializers: &Value,
    ) -> Result<(Value, Value), VmErr> {
        let access_state = Value::object(vec![("key".into(), key.clone())]);
        if private {
            let key = self.property_key(key)?;
            let id = scope
                .borrow()
                .private_name(&key)
                .expect("declared private element");
            access_state.set_prop("privateId".into(), Value::String(id.to_string().into()))?;
        }
        let mut access = Vec::new();
        if kind != "setter" {
            access.push((
                "get".into(),
                crate::builtins::bound_native_method("", 1, access_get, None, access_state.clone()),
            ));
        }
        if matches!(kind, "field" | "accessor" | "setter") {
            access.push((
                "set".into(),
                crate::builtins::bound_native_method("", 2, access_set, None, access_state.clone()),
            ));
        }
        access.push((
            "has".into(),
            crate::builtins::bound_native_method("has", 1, access_has, None, access_state),
        ));
        let state = Value::object(vec![
            ("active".into(), Value::Bool(true)),
            ("initializers".into(), initializers.clone()),
        ]);
        let context = Value::object(vec![
            ("kind".into(), Value::String(kind.into())),
            ("access".into(), Value::object(access)),
            ("static".into(), Value::Bool(is_static)),
            ("private".into(), Value::Bool(private)),
            ("name".into(), key.clone()),
            (
                "addInitializer".into(),
                crate::builtins::bound_native_method(
                    "addInitializer",
                    1,
                    super::eval::decorator_add_initializer,
                    None,
                    state.clone(),
                ),
            ),
        ]);
        Ok((context, state))
    }

    pub(crate) fn decorate_class_fields(
        &mut self,
        scope: &Env,
        records: &[ClassFieldDefinition],
    ) -> Result<Vec<(usize, Value)>, VmErr> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let instance_states = Value::array(vec![]);
        let mut static_states = Vec::new();
        for is_static in [true, false] {
            for record in records
                .iter()
                .filter(|record| record.is_static == is_static)
            {
                let transforms = Value::array(vec![]);
                let extras = Value::array(vec![]);
                for (decorator, receiver) in record.decorators.iter().rev() {
                    let (context, active) = self.element_decorator_context(
                        scope,
                        "field",
                        &record.key,
                        record.private,
                        is_static,
                        &extras,
                    )?;
                    let result = self.call_this(
                        decorator,
                        receiver.clone(),
                        vec![Value::Undefined, context],
                    );
                    active.set_prop("active".into(), Value::Bool(false))?;
                    let transform = result?;
                    if super::is_callable_value(&transform) {
                        let list = transforms.as_array().expect("internal transform list");
                        list.store_index(list.array_length(), transform);
                    } else if !matches!(transform, Value::Undefined) {
                        return Err(VmErr::Msg(
                            "TypeError: field decorator must return a callable or undefined".into(),
                        ));
                    }
                }
                let state = Value::object(vec![
                    ("transforms".into(), transforms),
                    ("extras".into(), extras),
                ]);
                if is_static {
                    static_states.push((record.index, state));
                } else {
                    instance_states
                        .as_array()
                        .expect("internal field states")
                        .store_index(record.index, state);
                }
            }
        }
        scope.borrow_mut().declare(
            INSTANCE_FIELD_INITIALIZERS,
            instance_states,
            BindKind::Const,
            true,
        );
        Ok(static_states)
    }

    pub(crate) fn apply_field_initializers(
        &mut self,
        state: &Value,
        receiver: &Value,
        mut value: Value,
    ) -> Result<Value, VmErr> {
        let transforms = state
            .get_prop("transforms")
            .and_then(|value| value.as_array())
            .expect("internal transform list");
        for index in (0..transforms.array_length()).rev() {
            self.check_execution_interrupt()?;
            let function = transforms.index_value(index).expect("dense transform list");
            value = self.call_this(&function, receiver.clone(), vec![value])?;
        }
        Ok(value)
    }

    pub(crate) fn run_field_extra_initializers(
        &mut self,
        state: &Value,
        receiver: &Value,
    ) -> Result<(), VmErr> {
        self.run_method_initializers(
            &state
                .get_prop("extras")
                .expect("internal extra initializer list"),
            receiver,
        )
    }

    pub(crate) fn decorate_class_methods(
        &mut self,
        class: &Value,
        scope: &Env,
        records: &mut [ClassMethodDefinition],
        private_statics: &mut [(u64, PrivateElement)],
    ) -> Result<Option<Value>, VmErr> {
        if records.is_empty() {
            return Ok(None);
        }
        let static_initializers = Value::array(vec![]);
        let instance_initializers = Value::array(vec![]);
        for is_static in [true, false] {
            for record in records
                .iter_mut()
                .filter(|record| record.is_static == is_static)
            {
                if record.decorators.is_empty() {
                    continue;
                }
                let initializers = if is_static {
                    &static_initializers
                } else {
                    &instance_initializers
                };
                for (decorator, receiver) in record.decorators.iter().rev() {
                    let (context, state) = self.element_decorator_context(
                        scope,
                        match record.kind {
                            ClassMethodKind::Method => "method",
                            ClassMethodKind::Getter => "getter",
                            ClassMethodKind::Setter => "setter",
                        },
                        &record.key,
                        record.private,
                        is_static,
                        initializers,
                    )?;
                    let result = self.call_this(
                        decorator,
                        receiver.clone(),
                        vec![record.value.clone(), context],
                    );
                    state.set_prop("active".into(), Value::Bool(false))?;
                    let replacement = result?;
                    if super::is_callable_value(&replacement) {
                        record.value = replacement;
                    } else if !matches!(replacement, Value::Undefined) {
                        return Err(VmErr::Msg(
                            "TypeError: method decorator must return a callable or undefined"
                                .into(),
                        ));
                    }
                }
            }
        }
        // Replaying all definitions preserves source-order overrides, even when
        // an earlier overwritten method was decorated for its side effects.
        for record in records {
            if record.private {
                let key = self.property_key(&record.key)?;
                let id = scope
                    .borrow()
                    .private_name(&key)
                    .expect("declared private member");
                if record.is_static {
                    let (_, element) = private_statics
                        .iter_mut()
                        .find(|(candidate, _)| *candidate == id)
                        .expect("defined private member");
                    match record.kind {
                        ClassMethodKind::Method => {
                            *element = PrivateElement::Method(record.value.clone())
                        }
                        ClassMethodKind::Getter | ClassMethodKind::Setter => {
                            let PrivateElement::Accessor { get, set } = element else {
                                unreachable!("private accessor kind");
                            };
                            if record.kind == ClassMethodKind::Getter {
                                *get = Some(record.value.clone());
                            } else {
                                *set = Some(record.value.clone());
                            }
                        }
                    }
                } else if record.kind == ClassMethodKind::Method {
                    scope
                        .borrow_mut()
                        .replace_private_instance_method(id, record.value.clone());
                } else {
                    scope.borrow_mut().replace_private_instance_accessor(
                        id,
                        record.kind == ClassMethodKind::Setter,
                        record.value.clone(),
                    );
                }
                continue;
            }
            let target = if record.is_static {
                class.clone()
            } else {
                class.get_prop("prototype").expect("class prototype")
            };
            let mut fields = vec![
                ("enumerable".into(), Value::Bool(false)),
                ("configurable".into(), Value::Bool(true)),
            ];
            match record.kind {
                ClassMethodKind::Method => {
                    fields.push(("value".into(), record.value.clone()));
                    fields.push(("writable".into(), Value::Bool(true)));
                }
                ClassMethodKind::Getter => fields.push(("get".into(), record.value.clone())),
                ClassMethodKind::Setter => fields.push(("set".into(), record.value.clone())),
            }
            if !self.define_own_property(&target, &record.key, &Value::descriptor_record(fields))? {
                return Err(VmErr::Msg(
                    "TypeError: cannot install decorated method".into(),
                ));
            }
        }
        scope.borrow_mut().declare(
            INSTANCE_METHOD_INITIALIZERS,
            instance_initializers,
            BindKind::Const,
            true,
        );
        Ok(Some(static_initializers))
    }

    pub(crate) fn run_method_initializers(
        &mut self,
        list: &Value,
        receiver: &Value,
    ) -> Result<(), VmErr> {
        let list = list.as_array().expect("internal initializer list");
        for index in 0..list.array_length() {
            self.check_execution_interrupt()?;
            let initializer = list.index_value(index).expect("dense initializer list");
            self.call_this(&initializer, receiver.clone(), vec![])?;
        }
        Ok(())
    }
}
