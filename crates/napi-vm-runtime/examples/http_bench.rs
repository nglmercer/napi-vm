//! Native loopback HTTP worker benchmark. See bench/http.py.
#[cfg(not(target_arch = "wasm32"))]
#[path = "http_bench/native.rs"]
mod native;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::main();
}

#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("The HTTP transport benchmark requires a native target");
}
