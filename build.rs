//! Selects the generator / async-function implementation for the target.
//!
//! Both are built on `corosensei` stackful coroutines, which need
//! hand-written stack-switching assembly. `corosensei` only ships that
//! assembly for some targets and `compile_error!`s on the rest — notably
//! `aarch64-pc-windows-msvc`, whose aarch64 backend is gated `not(windows)`,
//! and `wasm32`, which has no addressable stack at all.
//!
//! Rather than spell that condition out at each of the ~50 `cfg` sites (and
//! get one of them wrong), this emits a single `stackful_coroutines` cfg.
//! Where it is absent, generators fall back to the buffered path documented
//! in `interpreter::call::generator_next` and `await` resolves eagerly.
//!
//! **This must stay in sync with the `corosensei` target sections in
//! `Cargo.toml`**: the crate is only a dependency where this returns true, so
//! claiming support that `Cargo.toml` does not ship fails to compile.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=native/node_api_shim.c");
    println!("cargo::rustc-check-cfg=cfg(stackful_coroutines)");
    println!("cargo::rustc-check-cfg=cfg(node_api_host_unavailable)");

    if std::env::var_os("CARGO_FEATURE_NODE_API_HOST").is_some() {
        let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        if target_os == "linux" {
            let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
            let library = out_dir.join("libnapi_vm_node_api_shim.so");
            let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
            let status = std::process::Command::new(compiler)
                .args(["-std=c11", "-O2", "-fPIC", "-fvisibility=hidden", "-shared"])
                .arg("native/node_api_shim.c")
                .arg("-o")
                .arg(&library)
                .status()
                .expect("node-api-host requires a C compiler");
            assert!(
                status.success(),
                "failed to compile the Node-API symbol shim"
            );
            println!(
                "cargo::rustc-env=NAPI_VM_NODE_API_SHIM_PATH={}",
                library.display()
            );
        } else {
            println!("cargo::rustc-cfg=node_api_host_unavailable");
        }
    }

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let windows = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows";

    let supported = match arch.as_str() {
        // corosensei has both a SysV and a Windows backend for these.
        "x86_64" | "x86" => true,
        // These have a SysV backend only. On Windows they hit
        // `compile_error!("Unsupported target")`.
        "aarch64" | "riscv32" | "riscv64" | "loongarch64" => !windows,
        // wasm32, and anything else we do not build for.
        _ => false,
    };

    if supported {
        println!("cargo::rustc-cfg=stackful_coroutines");
    }
}
