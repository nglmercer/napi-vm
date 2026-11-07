# Supported contract schema profile

Trusted-plugin schemas use a deliberately constrained data-only subset of
JSON Schema draft 2020-12. The generator rejects unsupported keywords/forms
instead of silently producing weaker validators or types.

## Documents and references

Schema documents contain `$schema`, `$defs`, and optional `title`/`description`.
The supported `$schema` URI identifies draft 2020-12. Interface descriptors
refer to named definitions; generation normalizes bounded local/external
references into contract definitions. External files must remain within the
contract directory. Reference cycles and excessive depth/file counts fail.
`$ref` can have descriptive metadata, not arbitrary constraint siblings.

## Supported data forms

| Form | Constraints |
| --- | --- |
| Closed object | Named `properties`, explicit `required`, `additionalProperties: false` |
| Array | One `items` schema; optional `minItems` / `maxItems` |
| String | Optional scalar-count `minLength` / `maxLength`, supported string enum/const, or wire annotation |
| Integer | Safe integer values with supported bounds/const |
| Number | Finite numeric values with supported bounds/const |
| Boolean or null | The declared primitive, with supported const where appropriate |
| Nullable | One non-null primitive/data type plus `null`; supported combinations are checked by the generator |
| Tagged union | `oneOf` with 2–32 closed object variants and a shared required, unique string-const discriminator |

Unknown object fields are invalid. Optional fields preserve absence. Nullable
means explicit `null` is allowed; it does not imply a field may be omitted.
Bounds and annotations are validated at runtime even when the generated static
type cannot express them.

## Special string encodings

Use `x-wire-type` on a string schema for these encodings:

| Annotation | Wire representation |
| --- | --- |
| `i64-decimal` | Canonical signed 64-bit decimal string |
| `u64-decimal` | Canonical unsigned 64-bit decimal string |
| `bytes-base64` | Canonical base64 text |
| `utc-datetime` | Valid Gregorian UTC timestamp with milliseconds, such as `2026-01-02T03:04:05.000Z` |

Raw BigInts, binary buffers, Date objects, functions, accessors, symbols, cycles,
sparse arrays, non-finite numbers, and unpaired Unicode surrogates are not wire
values. Normalization/validators enforce canonical encodings; a lookalike string
does not pass merely because its generated type is `string`.

## Unsupported general-schema features

Do not use arbitrary `pattern`, `format`, open dictionaries, tuple items,
`allOf`/`anyOf`/`not`, remote network references, or general recursive schemas.
This profile is intended for matching TypeScript/Rust message contracts, not
every schema expressible by the full JSON Schema specification.

The authoritative normalizer is
[plugin-codegen](../../packages/plugin-codegen/src/index.mjs); runtime validators
live in the [JavaScript protocol](../../packages/plugin-protocol/src/index.ts)
and [Rust protocol](../../crates/napi-vm-plugin-protocol/src/schema.rs).
See the [contract guide](../../docs/trusted-plugins/contracts.md) and
[wire-types example](generated/wire-types.md) for complete generated artifacts.
