//! Run an application entry point with explicitly allowlisted Node-API addons.
//!
//! The digest argument should come from trusted application metadata (for
//! example, a signed manifest), not from the addon package itself.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use napi_vm::{Interpreter, RustNodeApiOptions};

fn main() {
    if let Err(error) = run() {
        eprintln!("rust-native-addon: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let root = required_path(&mut args, "application root")?;
    let entry = required_path(&mut args, "CommonJS entry file")?;
    let addon = required_path(&mut args, "Node-API .node file")?;
    let digest = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| usage())?;
    if args.next().is_some() {
        return Err(usage());
    }
    let digest = parse_sha256(&digest)?;

    let source = fs::read_to_string(&entry)
        .map_err(|error| format!("cannot read {}: {error}", entry.display()))?;

    let mut vm = Interpreter::with_builtins();
    let runtime = vm
        .enable_native_addons(
            RustNodeApiOptions::new([root])
                .allow_native_addon_with_sha256(&addon, digest)
                .entry(&entry),
        )
        .map_err(|error| error.to_string())?;
    runtime
        .preflight_addon(&addon)
        .map_err(|error| error.to_string())?;

    vm.eval_source(&source).map_err(|error| error.to_string())?;
    runtime.shutdown().map_err(|error| error.to_string())?;
    Ok(())
}

fn required_path(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}; {}", usage()))
}

fn parse_sha256(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("SHA-256 must contain exactly 64 hexadecimal characters".into());
    }

    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "SHA-256 contains a non-hexadecimal character".to_string())?;
    }
    Ok(digest)
}

fn usage() -> String {
    "usage: rust-native-addon <app-root> <entry.cjs> <addon.node> <trusted-sha256>".into()
}
