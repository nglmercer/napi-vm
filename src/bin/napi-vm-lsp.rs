#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let code = napi_vm::lsp::run();
    std::process::exit(code);
}

// Cargo also discovers this binary while checking the browser library for
// wasm32. The language server has no browser-side process entry point.
#[cfg(target_arch = "wasm32")]
fn main() {}
