# @napi-vm/plugin-codegen

Generate matching TypeScript and Rust bindings, normalized contract artifacts,
and Markdown reference pages from interface descriptors and data schemas.
Generation is explicit and deterministic; a plugin host does not run it at load.

## Generate an interface

```sh
napi-vm-plugin-codegen path/to/greeter.interface.json generated
```

For checkout-wide outputs, use `npm run plugins:generate`. Verify committed
outputs with `npm run plugins:check-generated`.

The JavaScript API exposes `normalize`, `generate`, `generateAll`,
`generatePluginMetadata`, and `canonical`. `generate` writes `.contract.json`,
`.ts`, `.rs`, and `.md` files; check mode rejects missing or changed outputs.
Generated clients/handlers use the protocol and SDK packages/crates.

## Contract rules

An interface names its methods, input/output references, declared errors, events,
and optional state. Schemas use the supported data-only JSON Schema profile.
Normalization rejects unsupported keywords/forms and confines external references
to the contract directory. Optional absence differs from nullable data.

The digest covers normalized descriptor/schema identity. Hosts and plugins must
agree on exact version and digest. Static types do not replace runtime schema
validation, and idempotency metadata does not enable automatic retries.

## Develop

```sh
node --test packages/plugin-codegen/test/*.test.mjs
npm run plugins:typecheck
```

Run commands at the repository root after installing/building workspace
prerequisites. Change the generator or descriptor and regenerate; do not maintain
handwritten patches to generated references.

[Contract guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/contracts.md) ·
[Schema profile](https://github.com/nglmercer/napi-vm/blob/main/contracts/trusted-plugins/schema-profile.md)
