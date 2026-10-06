#![cfg(feature = "runtime")]
use napi_vm::runtime::{
    ExternalEventQueue, RuntimeBuilder, RuntimeLimits, permissions::Permissions,
};
use napi_vm::{Value, VmErr};
use std::sync::mpsc::TrySendError;
use std::time::Duration;
static_assertions::assert_impl_all!(napi_vm::runtime::ExternalEventSender: Send, Sync);
static_assertions::assert_not_impl_any!(napi_vm::runtime::Runtime: Send, Sync);

#[test]
fn runtime_globals_require_builder_opt_in() {
    let mut bare = RuntimeBuilder::new().build_runtime().unwrap();
    assert!(
        matches!(bare.eval("typeof setTimeout").unwrap(), Value::String(ref s) if s == "undefined")
    );
    let mut enabled = RuntimeBuilder::new()
        .timers()
        .console()
        .build_runtime()
        .unwrap();
    assert!(
        matches!(enabled.eval("typeof setTimeout").unwrap(), Value::String(ref s) if s == "function")
    );
    assert!(
        matches!(enabled.eval("typeof fetch").unwrap(), Value::String(ref s) if s == "undefined")
    );
}

#[test]
fn worker_completion_settles_on_owner_and_follows_microtasks() {
    let mut runtime = RuntimeBuilder::new().build_runtime().unwrap();
    let (id, promise) = runtime.external_promise().unwrap();
    runtime
        .interpreter_mut()
        .global
        .borrow_mut()
        .set("pending", promise);
    runtime
        .eval("var result = 0; pending.then(value => result = value)")
        .unwrap();
    let sender = runtime.external_sender();
    std::thread::spawn(move || sender.complete(id, Ok(serde_json::json!(42))).unwrap())
        .join()
        .unwrap();
    runtime.run_event_loop().unwrap();
    assert!(matches!(
        runtime.eval("result").unwrap(),
        Value::Number(42.0)
    ));
}

#[test]
fn completion_wakes_sleeping_owner_and_pending_roots_survive_gc() {
    let mut runtime = RuntimeBuilder::new().build_runtime().unwrap();
    let (id, promise) = runtime.external_promise().unwrap();
    runtime
        .interpreter_mut()
        .global
        .borrow_mut()
        .set("pending", promise);
    runtime
        .eval("var result; pending.then(value => result = value)")
        .unwrap();
    runtime.interpreter_mut().set_collection_threshold(1);
    runtime
        .eval("var garbage = []; for (var i = 0; i < 100; i++) garbage.push({i}); garbage = null;")
        .unwrap();
    let sender = runtime.external_sender();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        sender
            .complete(id, Ok(serde_json::json!({"answer": 42})))
            .unwrap();
    });
    runtime.run_event_loop().unwrap();
    worker.join().unwrap();
    assert!(matches!(
        runtime.eval("result.answer").unwrap(),
        Value::Number(42.0)
    ));
}

#[test]
fn cancellation_ignores_late_completion() {
    let mut runtime = RuntimeBuilder::new().build_runtime().unwrap();
    let (id, promise) = runtime.external_promise().unwrap();
    runtime
        .interpreter_mut()
        .global
        .borrow_mut()
        .set("pending", promise);
    runtime
        .eval("var result; pending.catch(error => result = error)")
        .unwrap();
    assert!(runtime.cancel_external(id));
    assert!(!runtime.cancel_external(id));
    runtime
        .external_sender()
        .complete(id, Ok(serde_json::json!(42)))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        matches!(runtime.eval("result").unwrap(), Value::String(ref s) if s.contains("AbortError"))
    );
}

#[test]
fn queue_backpressure_and_shutdown_are_explicit() {
    assert!(ExternalEventQueue::new(0).is_err());
    let queue = ExternalEventQueue::new(1).unwrap();
    let sender = queue.sender();
    sender.complete(1, Ok(serde_json::json!(1))).unwrap();
    assert!(matches!(
        sender.complete(2, Ok(serde_json::json!(2))),
        Err(TrySendError::Full(_))
    ));
    drop(queue);
    assert!(matches!(
        sender.complete(3, Ok(serde_json::json!(3))),
        Err(TrySendError::Disconnected(_))
    ));
    let mut runtime = RuntimeBuilder::new()
        .limits(RuntimeLimits {
            io_resources: 1,
            ..Default::default()
        })
        .build_runtime()
        .unwrap();
    runtime.external_promise().unwrap();
    assert!(
        matches!(runtime.external_promise(), Err(VmErr::Msg(ref s)) if s.contains("IO resources"))
    );
}

#[test]
fn idle_does_not_wait_for_future_timers() {
    let mut runtime = RuntimeBuilder::new().timers().build_runtime().unwrap();
    runtime
        .eval("var done = false; setTimeout(() => done = true, 15)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(matches!(runtime.eval("done").unwrap(), Value::Bool(false)));
    runtime.run_event_loop().unwrap();
    assert!(matches!(runtime.eval("done").unwrap(), Value::Bool(true)));
}

#[test]
fn permissions_are_denied_by_default_and_scoped() {
    let denied = Permissions::new();
    assert!(denied.check_net("example.com", 443).is_err());
    assert!(denied.check_process().is_err());
    assert!(denied.check_ffi().is_err());
    assert!(denied.environment("PATH").is_err());
    let granted = Permissions::new().allow_net("EXAMPLE.COM", Some(443));
    assert!(granted.check_net("example.com", 443).is_ok());
    assert!(granted.check_net("example.com", 80).is_err());
    assert!(granted.check_net("sub.example.com", 443).is_err());
}

#[cfg(all(feature = "runtime-fs", unix))]
#[test]
fn filesystem_denies_traversal_symlinks_and_ungranted_operations() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("napi-vm-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("value.txt");
    std::fs::write(&file, "42").unwrap();
    symlink(&file, root.join("link.txt")).unwrap();
    std::fs::hard_link(&file, root.join("hard.txt")).unwrap();
    let policy = Permissions::new().allow_read(&root).unwrap();
    assert!(
        policy.read_text(&file, 10).is_err(),
        "hardlinked reads fail closed"
    );
    std::fs::remove_file(root.join("hard.txt")).unwrap();
    assert_eq!(policy.read_text(&file, 10).unwrap(), "42");
    assert!(policy.read_text(&file, 1).is_err());
    assert!(policy.read_text(root.join("link.txt"), 10).is_err());
    assert!(policy.read_text(root.join("../value.txt"), 10).is_err());
    assert!(policy.write_text(&file, "broken", 10).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "42");
    let mut runtime = RuntimeBuilder::new()
        .filesystem(policy)
        .build_runtime()
        .unwrap();
    let quoted = serde_json::to_string(file.to_str().unwrap()).unwrap();
    assert!(
        matches!(runtime.eval(&format!("napiVm.readTextFile({quoted})")).unwrap(), Value::String(ref s) if s == "42")
    );
    assert!(
        runtime
            .eval(&format!("napiVm.writeTextFile({quoted}, 'broken')"))
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn intervals_repeat_and_can_cancel_inside_callback() {
    let mut runtime = RuntimeBuilder::new().timers().build_runtime().unwrap();
    runtime.eval("var count = 0; var id = setInterval(() => { count++; if (count === 3) clearInterval(id); }, 1)").unwrap();
    runtime.run_event_loop().unwrap();
    assert!(matches!(runtime.eval("count").unwrap(), Value::Number(3.0)));
}

#[test]
fn timer_and_elapsed_limits_are_enforced() {
    let mut runtime = RuntimeBuilder::new()
        .timers()
        .limits(RuntimeLimits {
            timers: 1,
            ..Default::default()
        })
        .build_runtime()
        .unwrap();
    runtime.eval("setTimeout(() => {}, 1000)").unwrap();
    assert!(runtime.eval("setTimeout(() => {}, 1000)").is_err());
    let mut runtime = RuntimeBuilder::new()
        .limits(RuntimeLimits {
            timeout: Some(Duration::from_millis(5)),
            ..Default::default()
        })
        .build_runtime()
        .unwrap();
    runtime.external_promise().unwrap();
    runtime.eval("1").unwrap();
    // The VM has no runnable jobs, but pending external IO retains the deadline.
    std::thread::sleep(Duration::from_millis(10));
    assert!(runtime.run_event_loop().is_err());
}

#[test]
fn stale_completions_do_not_hide_ready_work_after_a_bounded_batch() {
    let mut runtime = RuntimeBuilder::new().build().unwrap();
    let (id, promise) = runtime.external_promise().unwrap();
    runtime
        .interpreter_mut()
        .global
        .borrow_mut()
        .set("pending", promise);
    runtime
        .eval("var result = 0; pending.then(value => result = value)")
        .unwrap();
    let sender = runtime.external_sender();
    for _ in 0..80 {
        sender.complete(0, Ok(serde_json::json!(0))).unwrap();
    }
    sender.complete(id, Ok(serde_json::json!(42))).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(matches!(
        runtime.eval("result").unwrap(),
        Value::Number(42.0)
    ));
}

#[test]
fn executing_intervals_and_atomics_timeouts_share_the_timer_cap() {
    let mut runtime = RuntimeBuilder::new()
        .timers()
        .limits(RuntimeLimits {
            timers: 1,
            ..Default::default()
        })
        .build()
        .unwrap();
    runtime.eval("var blocked = false; var id = setInterval(() => { try { setTimeout(() => {}, 0); } catch (e) { blocked = true; } clearInterval(id); }, 1)").unwrap();
    runtime.run_event_loop().unwrap();
    assert!(matches!(
        runtime.eval("blocked").unwrap(),
        Value::Bool(true)
    ));
    runtime.eval("var words = new Int32Array(new SharedArrayBuffer(4)); Atomics.waitAsync(words, 0, 0, 10);").unwrap();
    assert!(runtime.eval("setTimeout(() => {}, 0)").is_err());
}

#[cfg(all(feature = "runtime-fs", feature = "runtime-net", unix))]
#[test]
fn explicit_filesystem_and_network_grants_combine() {
    let root = std::env::temp_dir();
    let path = root.join(format!("napi-vm-grant-merge-{}", std::process::id()));
    std::fs::write(&path, "retained read grant").unwrap();
    let mut runtime = RuntimeBuilder::new()
        .filesystem(Permissions::new().allow_read(&root).unwrap())
        .network(Permissions::new().allow_net("example.com", Some(443)))
        .build()
        .unwrap();
    let source = format!(
        "napiVm.readTextFile({})",
        serde_json::to_string(&path.to_string_lossy()).unwrap()
    );
    assert!(
        matches!(runtime.eval(&source).unwrap(),Value::String(ref s) if s=="retained read grant")
    );
    std::fs::remove_file(path).unwrap();
    assert!(runtime.permissions().check_net("example.com", 443).is_ok());
    assert!(runtime.permissions().check_net("example.com", 80).is_err());
    let combined = Permissions::new()
        .allow_env("NAPI_VM_MERGE_TEST")
        .merge(Permissions::new().allow_net("example.com", Some(443)));
    assert!(combined.environment("NAPI_VM_MERGE_TEST").is_ok());
    assert!(combined.check_net("example.com", 443).is_ok());
    assert!(combined.check_process().is_err());
}
