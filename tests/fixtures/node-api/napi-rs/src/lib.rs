use napi_derive::napi;

#[napi]
pub fn add(left: i32, right: i32) -> i32 {
    left + right
}

#[napi]
pub fn concatenate(left: String, right: String) -> String {
    format!("{left}{right}")
}
