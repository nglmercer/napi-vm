# Define contracts and generate bindings

A contract contains a normalized interface descriptor, its referenced data
schemas, and a deterministic digest. Host and plugin agree on exact interface
version and digest before ordinary calls. The package version, protocol version,
and interface version serve different purposes.

## Author the source files

Define methods, declared errors, events, and optional state in an
`.interface.json` descriptor. Point `types` at a schema document using the
supported [schema profile](../../contracts/trusted-plugins/schema-profile.md).
For example, the [greeter descriptor](../../contracts/trusted-plugins/examples/greeter.interface.json)
names the `greet` input/output definitions from its
[types schema](../../contracts/trusted-plugins/examples/greeter.types.schema.json).

Descriptors use version 1. Each method declares `input`, `output`, and an
`errors` array (empty when there are no domain errors); `idempotent` is optional
metadata. References resolve to normalized definitions;
external schema files are bounded and confined to the contract directory.
Unsupported schema forms fail generation instead of being silently weakened.

## Generate and check

```sh
node packages/plugin-cli/src/index.mjs codegen \
  contracts/trusted-plugins/examples/greeter.interface.json \
  --out artifacts/generated-greeter
npm run plugins:generate
npm run plugins:check-generated
```

Outputs include `.contract.json`, `.ts`, `.rs`, and `.md`. The same normalization
produces the digest used at handshake and dispatch. TypeScript outputs provide
input/output types, registration helpers, clients, and error/event helpers.
Rust outputs provide types, contract helpers, handler traits, registration,
and clients. Markdown describes the same interface and wire data constraints.

| Example | Generated reference |
| --- | --- |
| Greeter method and domain error | [example.greeter](../../contracts/trusted-plugins/generated/greeter.md) |
| Counter events and state | [example.counter](../../contracts/trusted-plugins/generated/counter.md) |
| Application-provided configuration service | [app.configuration](../../contracts/trusted-plugins/generated/app-configuration.md) |
| Special wire values and optional/nullable fields | [wire types](../../contracts/trusted-plugins/generated/wire-types.md) |

## Respect the data boundary

Wire values are data-only JSON. Numbers must be finite; integers must be safe
where an integer schema is used. Strings must contain paired Unicode surrogates
and count scalar values for length constraints. Arbitrary class instances,
getters, functions, symbols, sparse arrays, cycles, and hidden properties are
not wire messages. The JavaScript encoder does not execute `toJSON` or getters
to transform application objects.

Use declared string annotations for 64-bit integers, bytes, and UTC datetimes.
Optional absence differs from `null`; only nullable schema forms accept `null`.
Generated static types cannot express every bound, so runtime validators remain
required on both ends.

## Change an interface deliberately

Changing normalized methods, schemas, errors, events, or state changes its
identity/digest. Generate host and plugin adapters from the same reviewed input.
A matching semantic version string alone is insufficient. `idempotent` is
metadata; peers do not automatically retry a request after timeout/disconnect.
Design application-level deduplication for effects that may outlive a deadline.

For state migration, maintain the state contract/version and provide an explicit
migration strategy. The Rust/TypeScript counter examples exercise replacement
across languages, but they do not make arbitrary implementation state portable.
