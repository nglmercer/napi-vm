//! Run the repository's plugin fixture through the reusable Rust plugin host.
//!
//! Run with:
//! `cargo run --no-default-features --example rust-plugin-host -- examples/plugins/example-plugin`

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use napi_vm::{RustPluginHost, RustPluginHostOptions, RustPluginPolicy};

fn main() -> Result<(), Box<dyn Error>> {
    let plugin_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/plugins/example-plugin"));
    let policy = RustPluginPolicy::default()
        .grant_fs_read("config.json")
        .grant_fs_read("assets/**")
        .grant_fs_write("cache/**")
        .grant_path();
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy,
        ..RustPluginHostOptions::default()
    });

    let plugin = host.load(&plugin_dir)?;
    let name = plugin.manifest.name.clone();
    println!(
        "loaded {}@{}; onLoad returned {:?}",
        plugin.manifest.name, plugin.manifest.version, plugin.load_result
    );

    let reloaded = host.reload(&name)?;
    println!(
        "reloaded {name}; onReload returned {:?}",
        reloaded.load_result
    );

    let state = host.unload(&name)?;
    println!("unloaded {name}; onUnload returned {state:?}");

    let status = fs::read_to_string(plugin_dir.join("cache/status.json"))?;
    println!("plugin wrote cache/status.json: {}", status.trim());
    Ok(())
}
