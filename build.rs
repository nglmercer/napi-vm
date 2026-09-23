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
    println!("cargo::rerun-if-env-changed=CC");
    println!("cargo::rustc-check-cfg=cfg(stackful_coroutines)");
    println!("cargo::rustc-check-cfg=cfg(node_api_host_unavailable)");

    if std::env::var_os("CARGO_FEATURE_NODE_API_HOST").is_some() {
        let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        if target_os == "linux" || target_os == "macos" || target_os == "windows" {
            let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
            let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
            let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
            let library_name = match target_os.as_str() {
                "macos" => "libnapi_vm_node_api_shim.dylib",
                // Windows Node-API addon import libraries name node.exe as
                // their provider, so the shim image must carry that module
                // name even though it is a DLL image.
                "windows" => "node.exe",
                _ => "libnapi_vm_node_api_shim.so",
            };
            let library = out_dir.join(library_name);
            let target = std::env::var("TARGET").unwrap_or_default();
            let host = std::env::var("HOST").unwrap_or_default();
            let target_specific_cc = std::env::var_os(format!("CC_{target}"))
                .or_else(|| std::env::var_os(format!("CC_{}", target.replace('-', "_"))));
            let compiler = target_specific_cc.unwrap_or_else(|| {
                if target_os == "windows" && target_env == "gnu" && target != host {
                    format!("{target_arch}-w64-mingw32-gcc").into()
                } else if target_os == "windows" && target_env == "msvc" {
                    std::env::var_os("CC").unwrap_or_else(|| "cl.exe".into())
                } else if target_os == "windows" && target_env == "gnu" {
                    std::env::var_os("CC").unwrap_or_else(|| "gcc".into())
                } else {
                    std::env::var_os("CC").unwrap_or_else(|| "cc".into())
                }
            });
            let mut command = std::process::Command::new(compiler);
            if target_os == "windows" && target_env == "msvc" {
                command
                    .args(["/nologo", "/O2", "/LD", "/TC"])
                    .arg("native/node_api_shim.c")
                    .arg(format!(
                        "/Fo{}",
                        out_dir.join("node_api_shim.obj").display()
                    ))
                    .arg("/link")
                    .arg(format!("/IMPLIB:{}", out_dir.join("node.lib").display()))
                    .arg(format!("/OUT:{}", library.display()));
            } else {
                let link_flag = if target_os == "macos" {
                    "-dynamiclib"
                } else {
                    "-shared"
                };
                command.args(["-std=c11", "-O2", "-fvisibility=hidden", link_flag]);
                if target_os != "windows" {
                    command.arg("-fPIC");
                } else {
                    command.arg(format!(
                        "-Wl,--out-implib,{}",
                        out_dir.join("libnode.exe.a").display()
                    ));
                }
                command
                    .arg("native/node_api_shim.c")
                    .arg("-o")
                    .arg(&library);
            }
            let status = command
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
