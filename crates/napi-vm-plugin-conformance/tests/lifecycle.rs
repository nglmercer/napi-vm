use napi_vm_plugin_host::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
struct Package(PathBuf);
impl Drop for Package {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn contract() -> Contract {
    Contract::from_value(
        serde_json::from_str(include_str!(
            "../../../contracts/trusted-plugins/generated/counter.contract.json"
        ))
        .unwrap(),
    )
    .unwrap()
}
fn package() -> Package {
    static NEXT_PACKAGE: AtomicU64 = AtomicU64::new(0);
    // Wall-clock resolution can give parallel tests identical timestamps.
    // Claim each directory atomically, also tolerating leftovers after a crash.
    let root = loop {
        let root = std::env::temp_dir().join(format!(
            "napi-rust-conformance-{}-{}",
            std::process::id(),
            NEXT_PACKAGE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&root) {
            Ok(()) => break root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("could not create isolated fixture directory: {error}"),
        }
    };
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir(root.join("contracts")).unwrap();
    let exe = if cfg!(windows) {
        "bin/plugin.exe"
    } else {
        "bin/plugin"
    };
    std::fs::copy(env!("CARGO_BIN_EXE_trusted-fixture-rust"), root.join(exe)).unwrap();
    let manifest = json!({"manifestVersion":2,"execution":"trusted-process","id":"example.counter","version":"0.1.0","protocol":{"major":1,"minMinor":0,"maxMinor":0},"provides":{"example.counter":"1.0.0"},"requiresHost":{},"profile":"native-executable","launch":{"kind":"executable","entry":exe,"args":[],"target":Target::current()},"assets":[],"contracts":["contracts/counter.contract.json"]});
    std::fs::write(
        root.join("plugin.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("contracts/counter.contract.json"),
        serde_json::to_vec(&contract()).unwrap(),
    )
    .unwrap();
    let mut files = serde_json::Map::new();
    for f in ["plugin.json", exe, "contracts/counter.contract.json"] {
        files.insert(
            f.into(),
            Value::String(format!(
                "{:x}",
                Sha256::digest(std::fs::read(root.join(f)).unwrap())
            )),
        );
    }
    let lock = json!({"lockVersion":1,"pluginId":"example.counter","pluginVersion":"0.1.0","artifact":{"profile":"native-executable","target":Target::current(),"abi":"native-executable","runtime":[],"availability":{"built":true,"distributed":false,"tested":[]}},"interfaces":{"example.counter":contract().identity()},"files":files});
    std::fs::write(
        root.join("plugin.lock.json"),
        serde_json::to_vec(&lock).unwrap(),
    )
    .unwrap();
    Package(root)
}
fn options(mode: &str) -> LoadOptions {
    LoadOptions {
        configuration: json!({"mode":mode}),
        ..Default::default()
    }
}
fn host() -> Host {
    Host::new(Limits {
        call_timeout_ms: 300,
        shutdown_timeout_ms: 150,
        startup_timeout_ms: 3000,
        max_log_bytes: 1024,
        ..Default::default()
    })
    .unwrap()
}
#[tokio::test]
async fn javascript_preflight_uses_a_relative_entry_and_preserves_arguments() {
    let pkg = package();
    let manifest_path = pkg.0.join("plugin.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let entry = "-entry 日本語 file.js";
    std::fs::write(pkg.0.join(entry), b"// controlled preflight fixture").unwrap();
    manifest["profile"] = json!("portable-js");
    manifest["launch"] = json!({"kind":"javascript","entry":entry,"runtimes":["node"],"preferredRuntime":"node","args":["argument with spaces"]});
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let opts = RuntimeOptions {
        runtime_paths: [(
            "node".into(),
            env!("CARGO_BIN_EXE_trusted-fixture-rust").into(),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let prepared = prepare(&manifest_path, &opts).await.unwrap();
    assert_eq!(prepared.selected_runtime, "node");
    assert_eq!(prepared.directory, std::fs::canonicalize(&pkg.0).unwrap());
    assert_eq!(prepared.arguments.len(), 2);
    assert_eq!(
        PathBuf::from(&prepared.arguments[0]),
        PathBuf::from(".").join(entry)
    );
    assert!(prepared.arguments[0].starts_with('.'));
    assert_eq!(prepared.arguments[1], "argument with spaces");
}

#[tokio::test]
async fn startup_failures_include_bounded_redacted_plugin_output() {
    let pkg = package();
    let h = host();
    let secret = "private-startup-fixture-credential";
    let mut opts = LoadOptions::default();
    opts.environment
        .insert("NAPI_VM_TEST_STARTUP_FAILURE".into(), secret.into());
    let error = match h.load(pkg.0.join("plugin.json"), opts).await {
        Ok(_) => panic!("fixture should exit before hello"),
        Err(error) => error,
    };
    assert_eq!(error.stable_code(), "PLUGIN_EXITED");
    assert_eq!(error.data["runtime"], "executable");
    let output: String = error.data["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|log| log["text"].as_str().unwrap())
        .collect();
    assert!(output.len() <= 1024);
    assert!(output.contains("startup fixture failure"));
    assert!(!output.contains(secret));
    assert_eq!(output.matches("[redacted]").count(), 2);
    assert!(error.data["droppedLogBytes"].as_u64().unwrap() > 0);
    assert_eq!(h.list()[0].status, Status::Failed);
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn repeated_unload_reaps_and_instances_are_independent() {
    let pkg = package();
    let h = host();
    let a = h
        .load(pkg.0.join("plugin.json"), options("normal"))
        .await
        .unwrap();
    let b = h
        .load(pkg.0.join("plugin.json"), options("normal"))
        .await
        .unwrap();
    assert_ne!(a.instance_id(), b.instance_id());
    assert_eq!(
        a.invoke(&contract(), "add", json!({"amount":12}))
            .await
            .unwrap(),
        json!({"count":"12"})
    );
    assert_eq!(
        b.invoke(&contract(), "get", json!({})).await.unwrap(),
        json!({"count":"0"})
    );
    #[cfg(target_os = "linux")]
    let pid = a.pid().unwrap();
    a.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
    assert_eq!(a.status(), Status::Stopped);
    assert!(a.pid().is_none());
    assert_eq!(
        a.invoke(&contract(), "get", json!({}))
            .await
            .unwrap_err()
            .stable_code(),
        "NOT_READY"
    );
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    h.shutdown().await.unwrap();
}
#[tokio::test]
async fn deadline_does_not_release_serial_slot_or_undo_effects() {
    let pkg = package();
    let h = host();
    let p = h
        .load(pkg.0.join("plugin.json"), options("ignore-cancel"))
        .await
        .unwrap();
    let e = p
        .context()
        .with_timeout(Duration::from_millis(15))
        .invoke(&contract(), "add", json!({"amount":1}))
        .await
        .unwrap_err();
    assert_eq!(e.stable_code(), "DEADLINE_EXCEEDED");
    assert_eq!(
        p.invoke(&contract(), "get", json!({}))
            .await
            .unwrap_err()
            .stable_code(),
        "REENTRANT_CALL"
    );
    let value = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match p.invoke(&contract(), "get", json!({})).await {
                Ok(v) => break v,
                Err(e) if e.stable_code() == "REENTRANT_CALL" => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(value, json!({"count":"1"}));
    h.shutdown().await.unwrap();
}
#[tokio::test]
async fn snapshot_failure_and_author_task_remain_draining() {
    for mode in ["snapshot-error", "uncooperative-task"] {
        let pkg = package();
        let h = host();
        let p = h
            .load(pkg.0.join("plugin.json"), options(mode))
            .await
            .unwrap();
        assert!(
            h.reload(&p.instance_id(), ReloadOptions::default())
                .await
                .is_err()
        );
        assert_eq!(p.status(), Status::Draining);
        assert_eq!(
            p.invoke(&contract(), "get", json!({}))
                .await
                .unwrap_err()
                .stable_code(),
            "NOT_READY"
        );
        let _ = h.shutdown().await;
        assert_eq!(p.status(), Status::Stopped);
    }
}
#[tokio::test]
async fn failed_initialize_cleanup_error_exit_and_noisy_pipes() {
    let pkg = package();
    let h = host();
    assert!(
        h.load(pkg.0.join("plugin.json"), options("initialize-error"))
            .await
            .is_err()
    );
    assert_eq!(h.list()[0].status, Status::Failed);
    let p = h
        .load(pkg.0.join("plugin.json"), options("shutdown-error"))
        .await
        .unwrap();
    assert!(h.unload(&p.instance_id()).await.is_err());
    assert_eq!(p.status(), Status::Stopped);
    let p = h
        .load(pkg.0.join("plugin.json"), options("logs"))
        .await
        .unwrap();
    assert_eq!(
        p.invoke(&contract(), "get", json!({})).await.unwrap(),
        json!({"count":"0"})
    );
    h.unload(&p.instance_id()).await.unwrap();
    let (logs, dropped) = p.logs();
    assert!(logs.iter().map(|x| x.text.len()).sum::<usize>() <= 1024);
    assert!(dropped > 0);
    let p = h
        .load(pkg.0.join("plugin.json"), options("exit-call"))
        .await
        .unwrap();
    assert!(
        p.invoke(&contract(), "add", json!({"amount":1}))
            .await
            .is_err()
    );
    let _ = h.shutdown().await;
}
#[tokio::test]
async fn tampered_inventory_rejected_before_execution() {
    let pkg = package();
    std::fs::write(pkg.0.join("contracts/counter.contract.json"), b"{}").unwrap();
    assert!(
        host()
            .load(pkg.0.join("plugin.json"), LoadOptions::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bootstrap_and_environment_credentials_never_enter_retained_output() {
    let pkg = package();
    let h = host();
    let secret = "fixture-private-environment-credential";
    let mut opts = options("secrets");
    opts.environment
        .insert("NAPI_VM_TEST_SECRET".into(), secret.into());
    let p = h.load(pkg.0.join("plugin.json"), opts).await.unwrap();
    // Graceful shutdown closes pipes and awaits their drainers before inspection.
    p.shutdown().await.unwrap();
    let (logs, _) = p.logs();
    let output: String = logs.iter().map(|x| x.text.as_str()).collect();
    assert!(
        output.contains("[redacted]"),
        "fixture output was not captured"
    );
    assert!(!output.contains(secret));
    assert_eq!(output.matches("[redacted]").count(), 4);
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn plugin_errors_and_instance_metadata_redact_known_credentials() {
    let pkg = package();
    let h = host();
    let secret = "private-error-credential";
    let mut opts = options("secret-error");
    opts.environment
        .insert("NAPI_VM_TEST_SECRET".into(), secret.into());
    let error = match h.load(pkg.0.join("plugin.json"), opts).await {
        Ok(_) => panic!("fixture should fail initialization"),
        Err(error) => error,
    };
    assert!(!serde_json::to_string(&error).unwrap().contains(secret));
    assert!(error.message.contains("[redacted]"));
    assert!(!serde_json::to_string(&h.list()).unwrap().contains(secret));
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_waits_for_starting_native_plugin_and_reaps_it() {
    let pkg = package();
    let h = host();
    let loading = h.clone();
    let manifest = pkg.0.join("plugin.json");
    let entered = pkg.0.join("initialize-entered");
    let release = pkg.0.join("initialize-release");
    let opts = LoadOptions {
        configuration: json!({"mode":"initialize-barrier", "entered":entered, "release":release}),
        ..Default::default()
    };
    let load = tokio::spawn(async move { loading.load(manifest, opts).await });
    // Package hashing precedes the protocol startup deadline. Wait for a fixture
    // signal instead of assuming how long hashing or native process launch takes.
    tokio::time::timeout(Duration::from_secs(30), async {
        while tokio::fs::metadata(&entered).await.is_err() {
            assert!(
                !load.is_finished(),
                "fixture failed before initialize barrier"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let metadata = h.list().into_iter().next().unwrap();
    assert_eq!(metadata.status, Status::Starting);
    let pid = h.get(&metadata.instance_id).unwrap().pid().unwrap();
    let stopping = h.clone();
    let shutdown = tokio::spawn(async move { stopping.shutdown().await });
    tokio::task::yield_now().await;
    assert!(
        !shutdown.is_finished(),
        "shutdown must wait for admitted initialization"
    );
    tokio::fs::write(&release, b"release").await.unwrap();
    let p = load.await.unwrap().unwrap();
    shutdown.await.unwrap().unwrap();
    assert_eq!(p.status(), Status::Stopped);
    assert!(p.pid().is_none());
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
    assert_eq!(
        p.invoke(&contract(), "get", json!({}))
            .await
            .unwrap_err()
            .stable_code(),
        "NOT_READY"
    );
    let error = match h
        .load(pkg.0.join("plugin.json"), LoadOptions::default())
        .await
    {
        Ok(_) => panic!("shutdown host admitted a new load"),
        Err(error) => error,
    };
    assert_eq!(error.stable_code(), "NOT_READY");
    p.shutdown().await.unwrap();
}
