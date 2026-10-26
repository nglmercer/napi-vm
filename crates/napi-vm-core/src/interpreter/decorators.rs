//! Class element decoration shared by both class frontends.
use super::{BindKind, Env, Interpreter};
use crate::error::VmErr;
use crate::value::{PrivateElement, Value};

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

impl Interpreter {
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
                let access_state = Value::object(vec![("key".into(), record.key.clone())]);
                if record.private {
                    let key = self.property_key(&record.key)?;
                    let id = scope
                        .borrow()
                        .private_name(&key)
                        .expect("declared private method");
                    access_state
                        .set_prop("privateId".into(), Value::String(id.to_string().into()))?;
                }
                for (decorator, receiver) in record.decorators.iter().rev() {
                    let state = Value::object(vec![
                        ("active".into(), Value::Bool(true)),
                        ("initializers".into(), initializers.clone()),
                    ]);
                    let access = Value::object(vec![
                        (
                            "has".into(),
                            crate::builtins::bound_native_method(
                                "has",
                                1,
                                access_has,
                                None,
                                access_state.clone(),
                            ),
                        ),
                        (
                            "get".into(),
                            crate::builtins::bound_native_method(
                                "",
                                1,
                                access_get,
                                None,
                                access_state.clone(),
                            ),
                        ),
                    ]);
                    let context = Value::object(vec![
                        ("kind".into(), Value::String("method".into())),
                        ("name".into(), record.key.clone()),
                        ("static".into(), Value::Bool(is_static)),
                        ("private".into(), Value::Bool(record.private)),
                        ("access".into(), access),
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
                if record.kind == ClassMethodKind::Method {
                    let key = self.property_key(&record.key)?;
                    let id = scope
                        .borrow()
                        .private_name(&key)
                        .expect("declared private method");
                    if record.is_static {
                        let (_, element) = private_statics
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == id)
                            .expect("defined private method");
                        *element = PrivateElement::Method(record.value.clone());
                    } else {
                        scope
                            .borrow_mut()
                            .replace_private_instance_method(id, record.value.clone());
                    }
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
