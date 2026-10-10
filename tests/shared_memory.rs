use napi_vm::value::{SharedAtomicOp, SharedBuffer, SharedMemory, SharedWaitResult};
use napi_vm::{CancellationToken, Value};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use std::sync::mpsc;
use std::time::{Duration, Instant};

assert_impl_all!(SharedMemory: Send, Sync);
assert_not_impl_any!(Value: Send, Sync);
assert_not_impl_any!(SharedBuffer: Send, Sync);

#[test]
fn transferred_memory_has_local_identity_and_shared_bytes() {
    let source = SharedBuffer::zeroed(16).unwrap();
    let block = source.shared_memory().unwrap();
    let identity = block.identity();
    assert!(source.atomic_store(0, 4, 41));
    let result = std::thread::spawn(move || {
        let local = SharedBuffer::from_shared_memory(block);
        assert_eq!(local.wait_identity(), identity);
        assert_eq!(local.atomic_rmw(0, 4, SharedAtomicOp::Add, 1, 0), Some(41));
        local.write(8, &[1, 2, 3, 4]);
        local.identity()
    })
    .join()
    .unwrap();
    assert_ne!(source.identity(), result);
    assert_eq!(source.atomic_load(0, 4), Some(42));
    assert_eq!(source.read(8, 4), Some(vec![1, 2, 3, 4]));
}

#[test]
fn synchronous_waits_are_notified_across_owner_threads() {
    let source = SharedBuffer::zeroed(8).unwrap();
    let block = source.shared_memory().unwrap();
    let (started, ready) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let local = SharedBuffer::from_shared_memory(block);
        started.send(()).unwrap();
        local.wait(0, 4, 0, 2000.0)
    });
    ready.recv().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while source.notify(0, 1) == 0 {
        assert!(Instant::now() < deadline, "wait did not register");
        std::thread::yield_now();
    }
    assert_eq!(thread.join().unwrap(), Some(SharedWaitResult::Ok));
    assert_eq!(source.notify(0, usize::MAX), 0);
}

#[test]
fn wait_registration_and_timeouts_do_not_leave_stale_notifications() {
    let source = SharedBuffer::zeroed(16).unwrap();
    assert_eq!(
        source.wait(0, 4, 1, f64::INFINITY),
        Some(SharedWaitResult::NotEqual)
    );
    assert_eq!(source.wait(0, 4, 0, 0.0), Some(SharedWaitResult::TimedOut));
    assert_eq!(source.wait(0, 4, 0, 1.0), Some(SharedWaitResult::TimedOut));
    let first = source
        .register_wait(0, 4, 0, f64::INFINITY)
        .unwrap()
        .unwrap();
    let other_offset = source
        .register_wait(4, 4, 0, f64::INFINITY)
        .unwrap()
        .unwrap();
    let second = source
        .register_wait(0, 8, 0, f64::INFINITY)
        .unwrap()
        .unwrap();
    assert_eq!(source.notify(0, 0), 0);
    assert_eq!(source.notify(0, 1), 1);
    assert!(first.is_notified());
    assert!(!second.is_notified());
    assert!(!other_offset.is_notified());
    drop(second);
    assert_eq!(source.notify(0, usize::MAX), 0);
    assert_eq!(source.notify(4, usize::MAX), 1);
}

#[test]
fn cancellation_releases_indefinite_blocking_waits() {
    let source = SharedBuffer::zeroed(8).unwrap();
    let block = source.shared_memory().unwrap();
    let cancellation = CancellationToken::default();
    let worker_cancellation = cancellation.clone();
    let thread = std::thread::spawn(move || {
        SharedBuffer::from_shared_memory(block).wait_cancellable(
            0,
            4,
            0,
            f64::INFINITY,
            &worker_cancellation,
        )
    });
    cancellation.cancel();
    assert_eq!(thread.join().unwrap(), Some(SharedWaitResult::Cancelled));
    assert_eq!(source.notify(0, usize::MAX), 0);
}

#[test]
fn atomics_use_identical_native_semantics_from_ast_and_bytecode() {
    let fixtures = [
        r#"var view = new Int8Array(new SharedArrayBuffer(8));
        var stored = Atomics.store(view, undefined, 257.9);
        var nan = Atomics.load(view, NaN);
        var replaced = Atomics.compareExchange(view, 0, 1, 130);
        var notified = Atomics.notify(new Int32Array(1), undefined);
        JSON.stringify([stored, nan, replaced, view[0], notified]);"#,
        r#"var view = new Int32Array(new SharedArrayBuffer(8));
        var immediate = Atomics.waitAsync(view, undefined, 1);
        var zero = Atomics.waitAsync(view, undefined, 0, 0);
        var pending = Atomics.waitAsync(view, undefined, 0);
        var notified = Atomics.notify(view, undefined, 1);
        JSON.stringify([immediate.async, immediate.value, zero.async, zero.value, pending.async, notified]);"#,
    ];
    for (fixture, expected) in fixtures.iter().zip([
        "[257,1,1,-126,0]",
        "[false,\"not-equal\",false,\"timed-out\",true,1]",
    ]) {
        let prepared = napi_vm::Interpreter::compile(fixture).unwrap();
        assert_eq!(
            prepared.tier(),
            napi_vm::interpreter::ExecutionTier::Bytecode
        );
        let mut bytecode = napi_vm::Interpreter::with_builtins();
        let result = bytecode.execute(&prepared).unwrap();
        assert!(
            matches!(result, Value::String(ref text) if text == expected),
            "{result:?}"
        );
        let mut parser = napi_vm::Parser::new(napi_vm::Lexer::new(fixture).tokenize());
        let statements = parser.parse_program().unwrap();
        let mut ast = napi_vm::Interpreter::with_builtins();
        let result = ast.run_program_body(&statements).unwrap();
        ast.drain_jobs().unwrap();
        assert!(
            matches!(result, Value::String(ref text) if text == expected),
            "{result:?}"
        );
    }
}

#[test]
fn blocking_atomics_wait_observes_execution_deadlines() {
    let mut vm = napi_vm::Interpreter::with_builtins();
    vm.set_can_block(true);
    vm.set_execution_timeout(Some(Duration::from_millis(30)));
    let started = Instant::now();
    let result = vm.eval_source("Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0)");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("deadline exceeded")
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn async_foreign_notification_wakes_the_standard_owner_event_loop() {
    let mut vm = napi_vm::Interpreter::with_builtins();
    vm.eval_source("var completed = false; var view = new Int32Array(new SharedArrayBuffer(4)); Atomics.waitAsync(view, 0, 0).value.then(function() { completed = true; });").unwrap();
    let buffer = vm.eval_source("view.buffer").unwrap();
    let Value::SharedArrayBuffer(ref buffer) = buffer else {
        panic!("expected SAB");
    };
    let memory = buffer.shared_memory().unwrap();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(SharedBuffer::from_shared_memory(memory).notify(0, 1), 1);
    });
    let started = Instant::now();
    assert!(vm.run_event_loop_once(Duration::from_secs(3)).unwrap());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "native notification did not wake the owner"
    );
    worker.join().unwrap();
    assert!(matches!(
        vm.global_value("completed"),
        Some(Value::Bool(true))
    ));
}

#[test]
fn imported_shared_memory_retains_the_receiving_realm_after_owner_drop() {
    let source = SharedBuffer::zeroed(4).unwrap();
    let mut parent = napi_vm::Interpreter::with_builtins();
    let child = parent.create_realm();
    let expected = child
        .global_value("SharedArrayBuffer")
        .unwrap()
        .get_prop("prototype")
        .unwrap();
    let imported = child.shared_array_buffer_from_memory(source.shared_memory().unwrap());
    drop(child);
    parent.global.borrow_mut().set("imported", imported);
    parent
        .global
        .borrow_mut()
        .set("expectedPrototype", expected);
    assert!(matches!(
        parent
            .eval_source("Object.getPrototypeOf(imported) === expectedPrototype")
            .unwrap(),
        Value::Bool(true)
    ));
    parent.collect_cycles();
    assert!(matches!(
        parent
            .eval_source("Object.getPrototypeOf(imported) === expectedPrototype")
            .unwrap(),
        Value::Bool(true)
    ));
}
