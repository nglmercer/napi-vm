# app.configuration

Generated contract reference. Regenerate this page with its TypeScript, Rust, and JSON artifacts; do not edit it by hand.

| Identity | Value |
| --- | --- |
| Interface version | `1.0.0` |
| Contract digest | `db06a9046edec0cff9afe336ca7bea7011133ac037f87dff207f652d2b47113f` |
| Descriptor version | 1 |

An asynchronous interface exchanging validated data-only messages.

Host and plugin must agree on the exact interface version and digest. A package version or matching method name alone is insufficient.

## Methods

### `get`

| Boundary | Type |
| --- | --- |
| Input | `GetInput` |
| Resolved output | `GetOutput` |

The JavaScript client returns a promise; the Rust client is asynchronous. Declared domain errors: none.

Idempotent metadata: `false`. Peers do not automatically retry calls.

## Data types

### `GetInput`

```typescript
type GetInput = { "key": string };
```

### `GetOutput`

```typescript
type GetOutput = { "value": string };
```

## Domain errors

No domain errors are declared.

## Events

No events are declared.

## Runtime validation

Generated validators enforce schema bounds and wire encodings in addition to static types. Optional fields preserve absence; null is accepted only where declared. Wire data rejects cycles, functions, non-finite numbers, and unpaired Unicode surrogates.

Use the generated client/registration helpers with the matching SDK. Cancellation and deadlines are cooperative and do not reverse completed application effects.
