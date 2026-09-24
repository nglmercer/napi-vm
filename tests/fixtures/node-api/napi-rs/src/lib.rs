use napi::bindgen_prelude::{AsyncTask, Buffer};
use napi::{Env, Error, Task};
use napi_derive::napi;

#[napi]
pub fn add(left: i32, right: i32) -> i32 {
    left + right
}

#[napi]
pub fn concatenate(left: String, right: String) -> String {
    format!("{left}{right}")
}

#[napi]
pub struct Counter {
    value: i32,
}

#[napi]
impl Counter {
    #[napi(constructor)]
    pub fn new(value: i32) -> Self {
        Self { value }
    }

    #[napi(getter)]
    pub fn value(&self) -> i32 {
        self.value
    }

    #[napi]
    pub fn increment(&mut self) -> i32 {
        self.value += 1;
        self.value
    }
}

#[napi]
pub fn reverse_bytes(input: Buffer) -> Buffer {
    Buffer::from(input.iter().rev().copied().collect::<Vec<_>>())
}

#[napi]
pub fn fail() -> napi::Result<i32> {
    Err(Error::from_reason("fixture failure"))
}

pub struct AddTask {
    left: i32,
    right: i32,
}

impl Task for AddTask {
    type Output = i32;
    type JsValue = i32;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        Ok(self.left + self.right)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

#[napi]
pub fn add_async(left: i32, right: i32) -> AsyncTask<AddTask> {
    AsyncTask::new(AddTask { left, right })
}
