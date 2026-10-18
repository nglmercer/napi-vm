use std::collections::{HashSet, VecDeque};

use super::Interpreter;
use crate::{error::VmErr, value::Value};

/// Private enumeration state shared by AST and bytecode execution.
pub(crate) struct ForInEnumerator {
    object: Option<Value>,
    keys: VecDeque<Value>,
    visited: HashSet<String>,
    started: bool,
    depth: usize,
}

impl ForInEnumerator {
    pub(crate) fn new(value: &Value) -> Result<Self, VmErr> {
        let object = if matches!(value, Value::Null | Value::Undefined) {
            None
        } else {
            Some(crate::builtins::object::to_object_receiver(value)?)
        };
        Ok(Self {
            object,
            keys: VecDeque::new(),
            visited: HashSet::new(),
            started: false,
            depth: 0,
        })
    }

    pub(crate) fn next(&mut self, interp: &mut Interpreter) -> Result<Option<String>, VmErr> {
        while let Some(object) = self.object.as_ref() {
            interp.consume_fuel(1)?;
            if !self.started {
                self.keys = interp.own_property_keys(object)?.into();
                self.started = true;
            }
            while let Some(key) = self.keys.pop_front() {
                interp.consume_fuel(1)?;
                let Value::String(name) = &key else {
                    continue;
                };
                let name = name.to_key();
                if self.visited.contains(&name) {
                    continue;
                }
                let descriptor =
                    crate::builtins::object::descriptor_for_key_in(interp, object, &key)?;
                if matches!(descriptor, Value::Undefined) {
                    continue;
                }
                self.visited.insert(name.clone());
                if descriptor
                    .get_prop("enumerable")
                    .is_some_and(|value| value.is_truthy())
                {
                    return Ok(Some(name));
                }
            }
            let parent = interp.get_prototype_of(object)?;
            if matches!(parent, Value::Null) {
                self.object = None;
            } else {
                self.depth += 1;
                if self.depth >= crate::value::MAX_PROTOTYPE_DEPTH {
                    return Err(crate::value::limit_err(
                        "Maximum prototype traversal depth exceeded",
                    ));
                }
                self.object = Some(parent);
                self.started = false;
            }
        }
        Ok(None)
    }
}
