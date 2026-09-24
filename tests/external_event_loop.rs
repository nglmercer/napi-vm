//! Exercise the VM from a current-thread async event loop, as a desktop host
//! would when it owns the UI thread and polls native callbacks each frame.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use napi_vm::{HostBridge, HostCallback, HostCallbackKind, HostEvent, Interpreter, Value, VmErr};

struct QueuedCallbackBridge {
    callback: Value,
    notifications: RefCell<Receiver<String>>,
}

impl HostBridge for QueuedCallbackBridge {
    fn call_host(&self, _id: usize, _args: Vec<Value>) -> Result<Value, VmErr> {
        Err(VmErr::Msg(
            "the test bridge has no synchronous calls".into(),
        ))
    }

    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        let notification = if timeout.is_zero() {
            self.notifications.borrow().try_recv().ok()
        } else {
            self.notifications.borrow().recv_timeout(timeout).ok()
        };
        Ok(notification
            .into_iter()
            .map(|message| {
                HostEvent::Callback(HostCallback {
                    callback: self.callback.clone(),
                    this_value: Value::Undefined,
                    args: vec![Value::String(message)],
                    kind: HostCallbackKind::Call,
                })
            })
            .collect())
    }
}

#[test]
fn tokio_current_thread_loop_delivers_host_callbacks_and_keeps_ticking() {
    let owner_thread = std::thread::current().id();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let mut vm = Interpreter::with_builtins();
        let callback = vm
            .eval_source(
                "globalThis.delivered = []; value => { delivered.push(value); queueMicrotask(() => delivered.push('microtask')); }",
            )
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        vm.set_host_bridge(Rc::new(QueuedCallbackBridge {
            callback,
            notifications: RefCell::new(receiver),
        }));
        let producer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            sender.send("native-event".to_string()).unwrap();
        });

        let ticks = Rc::new(Cell::new(0_u32));
        let tick_count = ticks.clone();
        let ticker = tokio::task::spawn_local(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(5));
            for _ in 0..30 {
                interval.tick().await;
                tick_count.set(tick_count.get() + 1);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut delivered = false;
        while Instant::now() < deadline {
            // A UI frame or async task can poll without blocking other work.
            vm.run_event_loop_once(Duration::ZERO).unwrap();
            if matches!(vm.eval_source("delivered.join(',')").unwrap(), Value::String(ref value) if value == "native-event,microtask") {
                delivered = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(delivered, "host callback and its microtask were not delivered");
        assert!(ticks.get() >= 3, "the external event loop stopped ticking");
        assert_eq!(std::thread::current().id(), owner_thread);
        producer.join().unwrap();
        ticker.await.unwrap();
    });
}

#[cfg(all(feature = "node-api-host", target_os = "linux"))]
#[test]
#[ignore = "build dist/rdev-node and run on an isolated X display with RDEV_NODE_TEST_LOOPBACK=1"]
fn tokio_loop_delivers_real_rdev_node_events_on_both_backends() {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use napi_vm::{NativeAddonOptions, NodeAddonOptions, RustNodeApiOptions};

    assert!(std::env::var_os("RDEV_NODE_TEST_LOOPBACK").is_some());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("dist/rdev-node")
        .canonicalize()
        .expect("build the vendored distribution first");
    let addon = root.join("node-rdev.linux-x64-gnu.node");
    assert!(addon.is_file());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        for backend in ["rust", "sidecar"] {
            let mut vm = Interpreter::with_builtins();
            let options = if backend == "rust" {
                NativeAddonOptions::RustNodeApi(
                    RustNodeApiOptions::new([root.clone()])
                        .allow_native_addon(&addon)
                        .entry(root.join("index.mjs")),
                )
            } else {
                NativeAddonOptions::NodeSidecar(
                    NodeAddonOptions::new("node", [root.clone()])
                        .allow_native_addon(&addon)
                        .entry(root.join("index.mjs")),
                )
            };
            let native_runtime = vm.enable_native_addons(options).unwrap();
            vm.eval_source("globalThis.rdev = require('./node-rdev.linux-x64-gnu.node'); globalThis.received = []; rdev.startListener(event => { received.push(event); queueMicrotask(() => { globalThis.microtasks = (globalThis.microtasks || 0) + 1; }); });").unwrap();

            // Input originates outside the VM, as it would in a desktop app.
            // Keep synchronous native calls off the Tokio owner thread.
            let mut producer = Command::new("node")
                .arg("-e")
                .arg("const addon=require(process.argv[1]); let count=0; const timer=setInterval(()=>{ const time=Date.now(); addon.simulateEvent({eventType:'KeyPress',keyPress:{key:'KeyB'},time}); addon.simulateEvent({eventType:'KeyRelease',keyRelease:{key:'KeyB'},time}); if(++count===30) clearInterval(timer); },75);")
                .arg(&addon)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();

            let tick_count = Rc::new(Cell::new(0_u32));
            let tick_count_task = tick_count.clone();
            let ticker = tokio::task::spawn_local(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(5));
                for _ in 0..100 {
                    interval.tick().await;
                    tick_count_task.set(tick_count_task.get() + 1);
                }
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut observed = false;
            while Instant::now() < deadline {
                vm.run_event_loop_once(Duration::ZERO).unwrap();
                if matches!(vm.eval_source("received.some(event => event.eventType === 'KeyPress' && event.keyPress && event.keyPress.key === rdev.KeyCode.KeyB) && (globalThis.microtasks || 0) > 0").unwrap(), Value::Bool(true)) {
                    observed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let _ = producer.kill();
            producer.wait().unwrap();
            vm.eval_source("rdev.stopListener()").unwrap();
            native_runtime.shutdown().unwrap();
            assert!(observed, "{backend} callback was not delivered");
            assert!(tick_count.get() >= 3, "Tokio stalled while {backend} was active");
            ticker.await.unwrap();
        }
    });
}
