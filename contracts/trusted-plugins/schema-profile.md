# Schema profile v1

The generator rejects unknown keywords with a file and JSON-pointer location. Objects have explicit properties/required and additionalProperties:false; arrays have one item schema; strings count Unicode scalar values. Integer values are safe binary64 integers. General numbers are finite binary64, with -0 normalized to 0. i64/u64, bytes and UTC datetimes use their explicit canonical string wire annotations.

Nullable syntax: `type:["string","null"]` (exactly one non-null primitive/container type). Tagged unions: `oneOf` inline object variants sharing a required string-const discriminator with distinct values. References: local relative files and JSON pointers only, at most 64 files and 32 reference edges, acyclic. All references are bundled into a normalized `$defs` tree. No recursive or remote references, coercion, regex, defaults, allOf or arbitrary dictionaries.

Canonical contract identity is SHA256 of UTF8 sorted-key JSON `{descriptor,schemas}` followed by one LF. Arrays retain order. The emitted contract artifact adds digest but that field is excluded from its own hash. Input source paths are excluded. Types, schemas, state, errors and event semantics are covered. `codegen --check` compares all required output bytes and reports absent output without writing.

Generated Rust `Field<T>` represents absence separately from `Option<T>` nullability. Runtime validation precedes typed deserialization, including duplicate-key last-value-wins parsing and normalization of integral binary64 values.

## Generated APIs and CLI configuration

Every named definition produces a typed TS validation function and Rust validation function. Rust validators first validate a `Value`, then normalize numeric storage through `from_wire_value`; optional nullable fields use `Field<Option<T>>`. Tagged string discriminators and enums are generated Rust enums. Punctuation and reserved identifiers are escaped; collisions receive deterministic suffixes. Wire property names remain unchanged, including `__proto__`.

The TS `client(target)` validates input and output. The generated `register(handlers, hooks?)` returns an SDK plugin definition with typed inputs, outputs and explicit `CallContext`. The Rust `register(&Registry, Arc<dyn Handler>)` is an object-safe boxed-future adapter. Clients do not create a runtime or connection.

`napi-vm-plugin codegen path/to/contract.interface.json --out generated --check` checks exact output bytes without updating destinations. Project-wide generation also rejects leftover generated files whose descriptor was removed. This check does not require a Git checkout. Check mode writes temporary regenerated files and removes them afterward.

For multiple interfaces, a CLI configuration has `contracts: [{input: "contracts/greeter.interface.json", out: "generated"}]`. Optional `plugin: {id, version, requiresHost: ["generated/app-configuration.contract.json"]}` and `metadataOut: "generated"` generate typed `PLUGIN_METADATA` and Rust `plugin_metadata()` adapters. Paths resolve relative to the configuration file. Metadata generation is explicit; ordinary Cargo compilation never runs JavaScript.

Schema root documents permit only `$schema`, `$defs`, `title`, and `description`. String, boolean, null and numeric literal constants are supported; constants must satisfy their other constraints. Combining nullable type syntax with enum/const is rejected in v1; use a separately designed tagged union instead. Inline tagged union variants cannot use sibling type/ref constraints. Generated definition count is limited to 1,024 and schema nesting to 64. Identifiers use lower-case ASCII letters/digits with dot or hyphen separators, up to 128 characters. Interface versions are SemVer including prerelease/build suffixes.

Rust general-number fields use `napi_vm_plugin_protocol::FiniteF64`, constructed with `FiniteF64::try_from(value)` and read with `.get()`. This rejects NaN/infinities before serialization, including inside optional/nullable arrays and fields, and normalizes negative zero. It prevents serde's non-finite-to-null conversion from changing a supplied value into permitted null. Generated TS clients accept `CallOptions`; handlers should bind generated clients to their explicit call context. Generated error helpers validate typed domain data, and TS event helpers validate payloads before emission.
