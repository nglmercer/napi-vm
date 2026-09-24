use napi::bindgen_prelude::AsyncTask;
use napi::{Env, Task};
use napi_derive::napi;

#[napi]
pub fn greet(name: String) -> String {
    format!("Hello, {name}!")
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
