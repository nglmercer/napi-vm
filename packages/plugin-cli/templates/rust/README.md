# Your Rust process plugin

Build a trusted native greeter executable with Cargo. The checked-in
`vendor/napi-vm-plugin-sdk` and protocol crate are independent licensed source
copies; Node/Bun/npm are not required to execute the native artifact.

## Build and test

From this generated project:

```sh
cargo check --locked
cargo test --locked
cargo build --release --locked
```

`src/lib.rs` contains business logic and tests; `src/main.rs` runs the SDK session.
`src/generated.rs` and the normalized contract are generated from definitions
under `contracts/`. Regenerate explicitly when the interface changes.

## Stage and package an artifact

With the plugin CLI and Cargo available:

```sh
napi-vm-plugin build . --format executable
napi-vm-plugin test . --artifact --host-executable /absolute/path/to/trusted-host-rust
napi-vm-plugin pack . --out ../packaged-plugin
```

The explicit build records the actual executable target and stages the declared
entry. The host executable above must be built for the machine running the test.
Normal host loading does not compile source. A successful build does not prove
another platform or ABI was tested.

Artifact scenarios are read from `plugin.test.json` in the selected test
directory. The test command above packages a temporary artifact. Include that
scenario file explicitly if you want to run the test from a distributed package.

The plugin has the launching user's OS authority. Track resources before
quiescing, validate state transfer, and implement cleanup. Cooperative deadlines
cannot reverse completed external effects. See the
[Rust authoring guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/authoring.md).
