# napi-vm-plugin-host

Native Tokio host for independent trusted process plugins. It launches declared native executables directly, or declared JavaScript plugins through Node/Bun. Native executable loading does not invoke JavaScript tooling.

```no_run
use napi_vm_plugin_host::{Host, LoadOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::default();
    let plugin = host.load("./my-plugin/plugin.json", LoadOptions::default()).await?;
    // Resolve contracts with plugin.contract(id), then invoke a declared method.
    plugin.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}
```

Register host callback implementations through `Host::register` before loading plugins that require them. `LoadOptions` carries runtime configuration, environment values and service connections; keep credentials out of package manifests and snapshots. Production loading verifies package integrity by default.

The host owns and reaps its direct child only. Arbitrary descendant containment requires OS-specific facilities. **Trusted plugins are not sandboxed** and inherit the launcher's OS privileges. Keep the `Host` alive while using handles; handles do not extend its ownership. After host drop, handle shutdown is a no-op. Shutdown is explicit; dropping the host is emergency cleanup, not a substitute for awaiting shutdown. Reload preserves instance handles while replacing sessions. Cancellation is cooperative, with no automatic retry or transactional rollback of external effects.

`services` provides explicitly opted-in native sidecar supervision with checksum/target preflight and loopback HTTP readiness. Sidecar output is drained and discarded; plugin retained output is bounded and known bootstrap, service-header and caller environment values and URL credentials are redacted, including known credentials in returned plugin errors. Avoid placing secrets in plugin-controlled errors or snapshots.

Startup handshake errors include the selected `runtime`, bounded and redacted `logs`, and `droppedLogBytes` in `RpcError.data`. Output pipes are drained before returning the error so CLI callers can report the plugin's final startup diagnostics.
