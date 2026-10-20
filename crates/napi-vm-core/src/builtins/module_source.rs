//! Realm-owned abstract base for host-provided module source objects.
use crate::interpreter::{Environment, Interpreter};
use crate::value::PropAttrs;
use crate::{Value, VmErr};
use std::rc::Rc;
pub(super) fn install(env: &mut Environment) {
    let function = env.get("Function").and_then(|v| v.get_prop("prototype"));
    let constructor =
        super::native_method("AbstractModuleSource", 0, unavailable, function.clone());
    super::make_callable(&constructor, unavailable, Some(unavailable));
    let prototype = Value::object_with_proto(
        vec![("constructor".into(), constructor.clone())],
        env.get("Object")
            .and_then(|v| v.get_prop("prototype"))
            .map(Rc::new),
    );
    prototype
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .set_attrs(
            "constructor",
            PropAttrs {
                writable: true,
                enumerable: false,
                configurable: true,
            },
        );
    let symbol = super::well_known("toStringTag").expect("well-known symbol");
    if let Value::Symbol(ref symbol) = symbol {
        let key = crate::interpreter::symbol_slot_key(symbol);
        prototype
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_symbol_key(&key, symbol.clone());
        let getter = super::native_method("get [Symbol.toStringTag]", 0, tag, function);
        prototype.set_prop(key.clone(), getter).unwrap();
        let properties = prototype.property_cell().unwrap();
        let mut metadata = properties.meta.borrow_mut();
        metadata.has_accessors = true;
        metadata.set_attrs(
            &key,
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    constructor.set_prop("prototype".into(), prototype).unwrap();
    constructor
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .set_attrs(
            "prototype",
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
    env.install_intrinsic("AbstractModuleSource", constructor);
}
fn unavailable(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(VmErr::Msg(
        "TypeError: AbstractModuleSource cannot be invoked directly".into(),
    ))
}
fn tag(_: &mut Interpreter, receiver: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let name = receiver
        .property_cell()
        .and_then(|properties| properties.meta.borrow().module_source_class_name.clone());
    Ok(name.map_or(Value::Undefined, Value::String))
}
