//! Probe the vendored rdev-node build through napi-vm's native loaders.
//! Build first with `bash vendor/rdev-node/build-dist.sh`.
//! Set RDEV_NODE_INJECT_INPUT=1 on an isolated X display to test callbacks.

use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[cfg(feature = "node-api-host")]
use napi_vm::RustNodeApiOptions;
use napi_vm::{Interpreter, NativeAddonOptions, NodeAddonOptions, Value, VmErr};

fn main() {
    if let Err(error) = run() {
        eprintln!("rdev-node probe: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), VmErr> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("dist/rdev-node")
        .canonicalize()
        .map_err(|error| VmErr::Msg(format!("build dist/rdev-node first: {error}")))?;
    let addons = std::fs::read_dir(&root)
        .map_err(|error| VmErr::Msg(error.to_string()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "node")
        })
        .collect::<Vec<_>>();
    let [addon] = addons.as_slice() else {
        return Err(VmErr::Msg(format!(
            "expected exactly one .node file in {}, found {}",
            root.display(),
            addons.len()
        )));
    };
    let backend = env::args().nth(1).unwrap_or_else(|| "rust".into());
    let mut vm = Interpreter::with_builtins();
    let options = match backend.as_str() {
        #[cfg(feature = "node-api-host")]
        "rust" => NativeAddonOptions::RustNodeApi(
            RustNodeApiOptions::new([root.clone()])
                .allow_native_addon(addon)
                .entry(root.join("index.mjs")),
        ),
        "sidecar" => NativeAddonOptions::NodeSidecar(
            NodeAddonOptions::new("node", [root.clone()])
                .allow_native_addon(addon)
                .entry(root.join("index.mjs")),
        ),
        _ => {
            return Err(VmErr::Msg(
                "backend must be sidecar, or rust with node-api-host enabled".into(),
            ));
        }
    };
    let runtime = vm.enable_native_addons(options)?;
    runtime.preflight_addon(addon)?;

    let entry = serde_json::to_string(&root.join("index.mjs").to_string_lossy())
        .map_err(|error| VmErr::Msg(error.to_string()))?;
    let request = serde_json::to_string(&format!(
        "./{}",
        addon.file_name().unwrap().to_string_lossy()
    ))
    .map_err(|error| VmErr::Msg(error.to_string()))?;
    vm.eval_source(&format!(
        "globalThis.localRequire = require('node:module').createRequire({entry}); globalThis.rdev = localRequire({request});"
    ))?;
    expect_true(
        &mut vm,
        &format!("rdev === localRequire(localRequire.resolve({request}))"),
    )?;
    expect_true(&mut vm, "rdev.isModifierKey(rdev.KeyCode.ControlLeft)")?;
    expect_true(&mut vm, "!rdev.isModifierKey(rdev.KeyCode.KeyA)")?;
    expect_true(
        &mut vm,
        "rdev.stringKeyToKeycode('a') === rdev.KeyCode.KeyA",
    )?;
    for name in [
        "startListener",
        "stopListener",
        "simulateEvent",
        "initSimulation",
    ] {
        expect_true(&mut vm, &format!("typeof rdev.{name} === 'function'"))?;
    }
    let simulation = vm.eval_source(
        "try { rdev.initSimulation(); 'ready' } catch (error) { 'error:' + error.message }",
    )?;
    println!(
        "backend={} simulation={simulation:?}",
        runtime.backend_name()
    );
    vm.eval_source("globalThis.events = []; globalThis.listenerErrors = []; rdev.startListener(event => events.push(event), message => listenerErrors.push(message));")?;
    expect_true(
        &mut vm,
        "try { rdev.startListener(() => {}); false } catch (error) { error.message.includes('already running') }",
    )?;

    let inject = env::var_os("RDEV_NODE_INJECT_INPUT").is_some();
    if inject {
        if !matches!(simulation, Value::String(ref value) if value == "ready") {
            return Err(VmErr::Msg(
                "input injection requested but the display is unavailable".into(),
            ));
        }
        wait_for_key(&mut vm)?;
        println!("first listener received KeyB");
    }
    expect_true(&mut vm, "listenerErrors.length === 0")?;
    expect_true(&mut vm, "rdev.stopListener() && !rdev.stopListener()")?;

    vm.eval_source("events = []; rdev.startListener(event => events.push(event), message => listenerErrors.push(message));")?;
    if inject {
        wait_for_key(&mut vm)?;
        println!("restarted listener received KeyB");
    }
    expect_true(&mut vm, "listenerErrors.length === 0")?;
    expect_true(&mut vm, "rdev.stopListener() && !rdev.stopListener()")?;
    runtime.shutdown()?;
    println!("rdev-node probe passed");
    Ok(())
}

fn expect_true(vm: &mut Interpreter, source: &str) -> Result<(), VmErr> {
    match vm.eval_source(source)? {
        Value::Bool(true) => Ok(()),
        actual => Err(VmErr::Msg(format!(
            "expected true from {source}, got {actual:?}"
        ))),
    }
}

fn wait_for_key(vm: &mut Interpreter) -> Result<(), VmErr> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        vm.eval_source("rdev.simulateEvent({eventType:rdev.EventTypeValue.KeyPress,keyPress:{key:rdev.KeyCode.KeyB},time:Date.now()}); rdev.simulateEvent({eventType:rdev.EventTypeValue.KeyRelease,keyRelease:{key:rdev.KeyCode.KeyB},time:Date.now()});")?;
        vm.run_event_loop_once(Duration::from_millis(250))?;
        if matches!(vm.eval_source("events.some(event => event.eventType === 'KeyPress' && event.keyPress && event.keyPress.key === rdev.KeyCode.KeyB)")?, Value::Bool(true)) {
            return Ok(());
        }
    }
    Err(VmErr::Msg(
        "listener did not receive a simulated KeyB press".into(),
    ))
}
