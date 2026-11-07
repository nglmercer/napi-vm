# example.wire-types

Generated contract reference. Regenerate this page with its TypeScript, Rust, and JSON artifacts; do not edit it by hand.

| Identity | Value |
| --- | --- |
| Interface version | `1.0.0` |
| Contract digest | `a0cdc912547e345bc9def20c0bb4110b60908cbcf4178fb68bea1e72734f2a7b` |
| Descriptor version | 1 |

An asynchronous interface exchanging validated data-only messages.

Host and plugin must agree on the exact interface version and digest. A package version or matching method name alone is insufficient.

## Methods

### `echo`

| Boundary | Type |
| --- | --- |
| Input | `WireTypes` |
| Resolved output | `WireTypes` |

The JavaScript client returns a promise; the Rust client is asynchronous. Declared domain errors: none.

Idempotent metadata: `false`. Peers do not automatically retry calls.

## Data types

### `WireTypes`

```typescript
type WireTypes = { "__proto__": string; "choice": "🌍" | "ready" | "quote\"\\value" | "literal\\u0010" | "controls\b\f"; "created": string; "data": string; "integer": number; "literal": 1; "optionalNull"?: string | null; "optionalNumber"?: number | null; "self"?: string | null; "self-field"?: string; "self_field"?: string; "tagged": ({ "kind": "text"; "value": string }) | ({ "kind": "number"; "value": number }); "wide": string };
```

## Domain errors

No domain errors are declared.

## Events

No events are declared.

## Runtime validation

Generated validators enforce schema bounds and wire encodings in addition to static types. Optional fields preserve absence; null is accepted only where declared. Wire data rejects cycles, functions, non-finite numbers, and unpaired Unicode surrogates.

Use the generated client/registration helpers with the matching SDK. Cancellation and deadlines are cooperative and do not reverse completed application effects.
