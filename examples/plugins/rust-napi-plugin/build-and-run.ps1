$ErrorActionPreference = 'Stop'

$pluginDir = (Resolve-Path $PSScriptRoot).Path
$repoRoot = (Resolve-Path (Join-Path $pluginDir '..\..\..')).Path
$nativeDir = Join-Path $pluginDir 'native'
$nativeAddon = Join-Path $nativeDir 'target\release\rust_napi_plugin_native.dll'
$addonPath = Join-Path $pluginDir 'node_modules\rust-napi-plugin.node\build\Release\addon.node'

Push-Location $repoRoot
try {
  cargo build --release --manifest-path (Join-Path $nativeDir 'Cargo.toml')
  if ($LASTEXITCODE -ne 0) {
    throw 'Could not build the Rust napi-rs addon.'
  }

  New-Item -ItemType Directory -Force -Path (Split-Path $addonPath) | Out-Null
  Copy-Item -Force $nativeAddon $addonPath
  $digest = (Get-FileHash $addonPath -Algorithm SHA256).Hash.ToLowerInvariant()

  cargo run --no-default-features --features node-api-host `
    --example rust-plugin-napi -- $pluginDir $digest
  if ($LASTEXITCODE -ne 0) {
    throw 'The Rust plugin host failed to load the napi-rs addon.'
  }
}
finally {
  Pop-Location
}
