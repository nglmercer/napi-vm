# napi-vm-plugin-protocol

Rust wire types, contract/schema validation, framing, errors, cancellation, and
asynchronous peer primitives for trusted process plugins. This crate is
independent of the JavaScript VM and does not launch a plugin.

Use `Contract` to load normalized generated contract JSON and validate method
inputs/results, domain errors, events, and state. Wire data is finite JSON with
Unicode-scalar strings. Declared string encodings represent 64-bit integers,
bytes, and UTC datetimes; arbitrary Rust/JavaScript object graphs are not wire
values. Optional absence and explicit null remain distinct.

Frames use a four-byte big-endian length followed by UTF-8 JSON. Peers enforce
bounds, response ordering, deadlines, and cancellation. Prefer the SDK/host
crates for lifecycle and process setup instead of assembling a peer manually.
Exact interface version/digest agreement is required.

## Develop

Run `cargo test -p napi-vm-plugin-protocol` from the repository root. Cross-language
changes also need the JavaScript and interoperability suites described in the
[testing guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/testing.md).

[Protocol specification](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/protocol.md) ·
[Contract authoring](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/contracts.md)
