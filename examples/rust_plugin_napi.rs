//! Run an ESM plugin that uses a napi-rs addon without embedding Node.
//!
//! Build the addon's cdylib, copy it into the plugin's `native/addon.node`,
//! calculate its trusted SHA-256, then run:
//! `cargo run --no-default-features --features node-api-host --example rust-plugin-napi -- <sha256>`

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use napi_vm::{RustPluginHost, RustPluginHostOptions, RustPluginNapiOptions, RustPluginPolicy};

fn main() {
    if let Err(error) = run() {
        eprintln!("rust-plugin-napi: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let default_plugin_dir = PathBuf::from("examples/plugins/rust-napi-plugin");
    let (plugin_dir, addon, digest) = match args.as_slice() {
        [digest] => (
            default_plugin_dir.clone(),
            PathBuf::from("native/addon.node"),
            digest,
        ),
        [plugin_dir, digest] => {
            let plugin_dir = PathBuf::from(plugin_dir);
            let addon = PathBuf::from("native/addon.node");
            (plugin_dir, addon, digest)
        }
        [plugin_dir, addon, digest] => (PathBuf::from(plugin_dir), PathBuf::from(addon), digest),
        _ => {
            return Err(
                "usage: rust-plugin-napi [plugin-dir [addon.node]] <trusted-sha256>".into(),
            );
        }
    };
    let digest = digest.to_str().ok_or("SHA-256 must be valid UTF-8")?;
    let digest = parse_sha256(digest)?;
    let plugin_dir = fs::canonicalize(plugin_dir)?;
    let addon = if addon.is_absolute() {
        addon
    } else {
        plugin_dir.join(addon)
    };
    let addon = fs::canonicalize(addon)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(plugin_dir.join("plugin.json"))?)?;
    let plugin_name = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or("plugin.json must contain a string name")?;

    let policy = RustPluginPolicy::default()
        .grant_fs_read("config.json")
        .grant_fs_write("cache/**")
        .grant_path();
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy,
        ..RustPluginHostOptions::default()
    });
    host.configure_napi_addons(
        plugin_name,
        RustPluginNapiOptions::default().allow_addon_with_sha256(&addon, digest),
    )?;

    let loaded = host.load(&plugin_dir)?;
    println!("loaded {}: {:?}", loaded.manifest.name, loaded.load_result);
    let name = loaded.manifest.name.clone();

    let reloaded = host.reload(&name)?;
    println!("reloaded {name}: {:?}", reloaded.load_result);

    let state = host.unload(&name)?;
    println!("unloaded {name}: {state:?}");
    Ok(())
}

fn parse_sha256(value: &str) -> Result<[u8; 32], Box<dyn Error>> {
    if value.len() != 64 {
        return Err("SHA-256 must contain exactly 64 hexadecimal characters".into());
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(digest)
}
