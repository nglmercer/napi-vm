//! Controlled failure fixture. Never runs arbitrary third-party code.
use napi_vm_plugin_protocol::Cancellation;
use napi_vm_plugin_sdk::*;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
fn contract() -> PluginResult<Contract> {
    Contract::from_value(serde_json::from_str(include_str!(
        "../../../../contracts/trusted-plugins/generated/counter.contract.json"
    ))?)
}
#[derive(Clone, Default)]
struct Hooks {
    mode: Arc<Mutex<String>>,
    count: Arc<Mutex<i64>>,
}
impl Lifecycle for Hooks {
    fn initialize(
        &self,
        config: Value,
        _: Value,
        snapshot: Value,
        context: CallContext,
    ) -> PluginFuture<'_, ()> {
        Box::pin(async move {
            let mode = config["mode"].as_str().unwrap_or("normal").to_string();
            *self.mode.lock().unwrap() = mode.clone();
            if mode == "initialize-barrier" {
                tokio::fs::write(config["entered"].as_str().unwrap(), b"entered").await?;
                while tokio::fs::metadata(config["release"].as_str().unwrap())
                    .await
                    .is_err()
                {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            if mode == "secret-error" {
                let value = std::env::var("NAPI_VM_TEST_SECRET").unwrap();
                let mut error = RpcError::new("INTERNAL_ERROR", value.clone());
                error.data["detail"] = json!({"nested":[value]});
                return Err(error);
            }
            if mode == "initialize-error" {
                return Err(RpcError::new(
                    "INTERNAL_ERROR",
                    "controlled initialize failure",
                ));
            }
            if !snapshot.is_null() {
                *self.count.lock().unwrap() =
                    snapshot["data"]["count"].as_str().unwrap().parse().unwrap();
            }
            if mode == "uncooperative-task" {
                context.resources().spawn(|_: Cancellation| {
                    Box::pin(async { std::future::pending::<()>().await })
                })?;
            }
            if mode == "secrets" {
                for name in ["NAPI_VM_PLUGIN_TOKEN", "NAPI_VM_TEST_SECRET"] {
                    let value = std::env::var(name).unwrap();
                    println!("secret={value}");
                    eprintln!("secret={value}");
                }
            }
            if mode == "logs" {
                for _ in 0..4096 {
                    println!("controlled noisy stdout abcdefghijklmnopqrstuvwxyz");
                    eprintln!("controlled noisy stderr abcdefghijklmnopqrstuvwxyz");
                }
            }
            Ok(())
        })
    }
    fn snapshot(&self, _: CallContext) -> PluginFuture<'_, Value> {
        Box::pin(async move {
            if *self.mode.lock().unwrap() == "snapshot-error" {
                return Err(RpcError::new(
                    "INTERNAL_ERROR",
                    "controlled snapshot failure",
                ));
            }
            Ok(
                json!({"stateVersion":1,"contract":"example.counter.state","data":{"count":self.count.lock().unwrap().to_string()}}),
            )
        })
    }
    fn shutdown(&self, _: CallContext) -> PluginFuture<'_, ()> {
        Box::pin(async move {
            if *self.mode.lock().unwrap() == "shutdown-error" {
                return Err(RpcError::new(
                    "INTERNAL_ERROR",
                    "controlled shutdown failure",
                ));
            }
            Ok(())
        })
    }
}
#[tokio::main]
async fn main() {
    // Controlled runtime probes let preflight tests inspect the actual launch
    // plan without depending on Node being installed for cargo test.
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("v24.19.0");
            return;
        }
        Some("-e") => {
            println!(
                "{}",
                json!({"versions":{"node":"24.19.0","napi":"8"},"target":napi_vm_plugin_host::Target::current()})
            );
            return;
        }
        _ => {}
    }
    if let Ok(secret) = std::env::var("NAPI_VM_TEST_STARTUP_FAILURE") {
        for _ in 0..256 {
            eprintln!("controlled noisy startup output abcdefghijklmnopqrstuvwxyz");
        }
        eprintln!(
            "startup fixture failure {secret} {}",
            std::env::var("NAPI_VM_PLUGIN_TOKEN").unwrap()
        );
        std::process::exit(7);
    }
    let result = async {
        let hooks = Hooks::default();
        let registry = Registry::new();
        let h = hooks.clone();
        registry.register(contract()?, "add", move |v, _| {
            let h = h.clone();
            Box::pin(async move {
                let mode = h.mode.lock().unwrap().clone();
                if mode == "exit-call" {
                    std::process::exit(7);
                }
                let n = {
                    let mut n = h.count.lock().unwrap();
                    *n += v["amount"].as_i64().unwrap();
                    *n
                };
                if mode == "ignore-cancel" {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                Ok(json!({"count":n.to_string()}))
            })
        })?;
        let h = hooks.clone();
        registry.register(contract()?, "get", move |_, _| {
            let h = h.clone();
            Box::pin(async move { Ok(json!({"count":h.count.lock().unwrap().to_string()})) })
        })?;
        serve(
            Plugin::new(registry).with_lifecycle(hooks),
            PluginMetadata {
                id: "example.counter".into(),
                version: "0.1.0".into(),
                requires_host: vec![],
            },
        )
        .await
    }
    .await;
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
