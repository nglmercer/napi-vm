# Generated contracts

The interface descriptor and JSON Schema type definitions are authoritative. Run `npm run plugins:generate` to emit TS/Rust types, client and registration adapters, runtime metadata, readable API docs and deterministic contract JSON. Run `npm run plugins:check-generated` to detect drift or missing outputs without rewriting them.

[Schema profile](../../contracts/trusted-plugins/schema-profile.md) defines the strict subset, canonical hashing and wire representation. General data is finite binary64, not arbitrary JSON-lossless serialization. Safe integers stay JSON numbers; 64-bit integers are canonical decimal strings, bytes canonical base64, timestamps strict Gregorian UTC milliseconds. Unicode lengths count scalar values. Missing and explicit null differ; Rust uses `Field<Option<T>>` where needed.

No public business type silently falls back to any/untyped JSON. Runtime dispatch uses validated normalized values and generated typed adapters; raw wire text is parsed last-key-wins before typed deserialization. Exact interface version plus digest is required, independently of plugin implementation version and protocol version.
