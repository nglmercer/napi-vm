//! Weak identities and ephemeron payloads. Closures below capture only weak
//! handles and scalar metadata; they never hold their referent strongly.
use super::*;
use std::rc::Weak;
#[derive(Clone)]
pub(crate) struct WeakTarget {
    id: usize,
    upgrade: Rc<dyn Fn() -> Option<Value>>,
}
impl std::fmt::Debug for WeakTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WeakTarget").field(&self.id).finish()
    }
}
impl WeakTarget {
    fn rc<T: 'static>(target: &Rc<T>, wrap: impl Fn(Rc<T>) -> Value + 'static) -> Self {
        let id = Rc::as_ptr(target) as usize;
        let weak = Rc::downgrade(target);
        Self {
            id,
            upgrade: Rc::new(move || weak.upgrade().map(&wrap)),
        }
    }
    pub(crate) fn new(value: &Value, global: &Env) -> Option<Self> {
        Some(match value {
            Value::Object { props } => Self::rc(props, |props| Value::Object { props }),
            Value::Array(array) => Self::rc(array, Value::Array),
            Value::Function(function) => Self::rc(function, Value::Function),
            Value::Promise(promise) => Self::rc(promise, Value::Promise),
            Value::Proxy(proxy) => Self::rc(proxy, Value::Proxy),
            Value::Generator { inner } => Self::rc(inner, |inner| Value::Generator { inner }),
            Value::StringIterator { inner } => {
                Self::rc(inner, |inner| Value::StringIterator { inner })
            }
            Value::Date(date) => Self::rc(date, Value::Date),
            Value::RegExp(regex) => Self::rc(regex, Value::RegExp),
            Value::TypedArray(view) => Self::rc(view, Value::TypedArray),
            Value::DataView(view) => Self::rc(view, Value::DataView),
            Value::ArrayBuffer(buffer) => {
                Self::rc(&buffer.0, |storage| Value::ArrayBuffer(Buffer(storage)))
            }
            Value::SharedArrayBuffer(buffer) => Self::rc(&buffer.0, |storage| {
                Value::SharedArrayBuffer(SharedBuffer(storage))
            }),
            Value::Symbol(symbol) if !crate::builtins::symbol_is_registered(symbol.id) => {
                Self::rc(symbol, Value::Symbol)
            }
            Value::HostFunction { name, properties } => {
                let name = name.clone();
                Self::rc(properties, move |properties| Value::HostFunction {
                    name: name.clone(),
                    properties,
                })
            }
            Value::NativeFunction { name, callable } => {
                let id = Rc::as_ptr(name) as *const () as usize;
                let name = Rc::downgrade(name);
                let callable = *callable;
                Self {
                    id,
                    upgrade: Rc::new(move || {
                        name.upgrade()
                            .map(|name| Value::NativeFunction { name, callable })
                    }),
                }
            }
            Value::Class(class) => {
                let constructor = Self::new(&class.constructor, global)?;
                let prototype = Rc::downgrade(&class.prototype);
                let name = class.name.clone();
                let statics = Rc::downgrade(&class.statics);
                Self {
                    id: Rc::as_ptr(&class.statics) as usize,
                    upgrade: Rc::new(move || {
                        Some(Value::Class(Box::new(ClassData {
                            name: name.clone(),
                            constructor: Box::new(constructor.upgrade()?),
                            prototype: prototype.upgrade()?,
                            statics: statics.upgrade()?,
                        })))
                    }),
                }
            }
            Value::Error(error) => {
                let properties = Rc::downgrade(&error.properties);
                let name = error.name.clone();
                let message = error.message.clone();
                let code = error.code.clone();
                let stack = error.stack.clone();
                Self::rc(&error.identity, move |identity| {
                    Value::Error(Box::new(ErrorData {
                        properties: properties.upgrade().expect("live error properties"),
                        identity,
                        name: name.clone(),
                        message: message.clone(),
                        code: code.clone(),
                        stack: stack.clone(),
                    }))
                })
            }
            Value::RealmGlobal(env) => Self::rc(env, |env| Value::RealmGlobal(env.clone())),
            Value::GlobalObject => Self::rc(global, |_| Value::GlobalObject),
            _ => return None,
        })
    }
    pub(crate) fn id(&self) -> usize {
        self.id
    }
    pub(crate) fn upgrade(&self) -> Option<Value> {
        (self.upgrade)()
    }
    pub(crate) fn matches(&self, value: &Value) -> bool {
        self.upgrade()
            .is_some_and(|key| crate::interpreter::strict_equals(&key, value))
    }
}
#[derive(Debug)]
pub(crate) struct FinalizationRecord {
    pub target: WeakTarget,
    pub held: Value,
    pub token: Option<WeakTarget>,
}
#[derive(Debug, Default)]
pub(crate) enum WeakStorage {
    #[default]
    None,
    Map(Vec<(WeakTarget, Value)>),
    Ref(Option<WeakTarget>),
    Registry {
        callback: Value,
        records: Vec<FinalizationRecord>,
        jobs: Weak<RefCell<crate::interpreter::JobQueue>>,
    },
}
impl WeakStorage {
    pub(crate) fn strong_values(&self, out: &mut Vec<Value>) {
        if let Self::Registry {
            callback, records, ..
        } = self
        {
            out.push(callback.clone());
            out.extend(records.iter().map(|r| r.held.clone()));
        }
    }
    pub(crate) fn drain_values(&mut self, out: &mut Vec<Value>) {
        match std::mem::take(self) {
            Self::Map(mut entries) => out.extend(entries.drain(..).map(|(_, value)| value)),
            Self::Registry {
                callback,
                mut records,
                ..
            } => {
                out.push(callback);
                out.extend(records.drain(..).map(|r| r.held));
            }
            _ => {}
        }
    }
}
impl Value {
    pub(crate) fn weak_identity(&self) -> Option<usize> {
        Some(match self {
            Value::RealmGlobal(global) => Rc::as_ptr(global) as usize,
            Value::Object { props } => Rc::as_ptr(props) as usize,
            Value::Array(value) => Rc::as_ptr(value) as usize,
            Value::Function(value) => Rc::as_ptr(value) as usize,
            Value::HostFunction { properties, .. } => Rc::as_ptr(properties) as usize,
            Value::NativeFunction { name, .. } => Rc::as_ptr(name) as *const () as usize,
            Value::Class(value) => Rc::as_ptr(&value.statics) as usize,
            Value::Promise(value) => Rc::as_ptr(value) as usize,
            Value::Generator { inner } => Rc::as_ptr(inner) as usize,
            Value::StringIterator { inner } => Rc::as_ptr(inner) as usize,
            Value::Proxy(value) => Rc::as_ptr(value) as usize,
            Value::Symbol(value) => Rc::as_ptr(value) as usize,
            Value::Date(value) => Rc::as_ptr(value) as usize,
            Value::RegExp(value) => Rc::as_ptr(value) as usize,
            Value::TypedArray(value) | Value::DataView(value) => Rc::as_ptr(value) as usize,
            Value::ArrayBuffer(value) => value.identity(),
            Value::SharedArrayBuffer(value) => value.identity(),
            Value::Error(value) => Rc::as_ptr(&value.identity) as usize,
            _ => return None,
        })
    }
}
