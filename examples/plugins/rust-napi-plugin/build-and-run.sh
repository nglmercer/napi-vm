#!/usr/bin/env bash
set -euo pipefail

plugin_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$plugin_dir/../../.." && pwd)
native_dir="$plugin_dir/native"
addon_path="$plugin_dir/node_modules/rust-napi-plugin.node/build/Release/addon.node"

case "$(uname -s)" in
  Linux*) native_name=librust_napi_plugin_native.so ;;
  Darwin*) native_name=librust_napi_plugin_native.dylib ;;
  MINGW*|MSYS*|CYGWIN*) native_name=rust_napi_plugin_native.dll ;;
  *) printf 'unsupported build host: %s\n' "$(uname -s)" >&2; exit 1 ;;
esac

cd -- "$repo_root"
cargo build --release --manifest-path "$native_dir/Cargo.toml"
mkdir -p "$(dirname "$addon_path")"
cp "$native_dir/target/release/$native_name" "$addon_path"

if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$addon_path" | cut -d ' ' -f 1)
elif command -v shasum >/dev/null 2>&1; then
  digest=$(shasum -a 256 "$addon_path" | cut -d ' ' -f 1)
else
  printf 'need sha256sum or shasum to pin the built addon\n' >&2
  exit 1
fi

cargo run --no-default-features --features node-api-host \
  --example rust-plugin-napi -- "$plugin_dir" "$digest"
