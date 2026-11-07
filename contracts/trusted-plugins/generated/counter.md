# example.counter

Generated contract reference. Regenerate this page with its TypeScript, Rust, and JSON artifacts; do not edit it by hand.

| Identity | Value |
| --- | --- |
| Interface version | `1.0.0` |
| Contract digest | `a6cc7c1e4c4ceedaaa1dde5626c052d5320c1b26ee21c63e3d581d4d972fc90f` |
| Descriptor version | 1 |

An asynchronous interface exchanging validated data-only messages.

Host and plugin must agree on the exact interface version and digest. A package version or matching method name alone is insufficient.

## Methods

### `add`

| Boundary | Type |
| --- | --- |
| Input | `AddInput` |
| Resolved output | `Count` |

The JavaScript client returns a promise; the Rust client is asynchronous. Declared domain errors: none.

Idempotent metadata: `false`. Peers do not automatically retry calls.

### `get`

| Boundary | Type |
| --- | --- |
| Input | `Empty` |
| Resolved output | `Count` |

The JavaScript client returns a promise; the Rust client is asynchronous. Declared domain errors: none.

Idempotent metadata: `true`. Peers do not automatically retry calls.

## Data types

### `AddInput`

```typescript
type AddInput = { "amount": number };
```

### `Count`

```typescript
type Count = { "count": string };
```

### `Empty`

```typescript
type Empty = {  };
```

## Domain errors

No domain errors are declared.

## Events

### `changed`

A contract-validated event.

Payload type: `Count`.

## State transfer

| Field | Value |
| --- | --- |
| Snapshot contract | `example.counter.state` |
| State version | 1 |
| Data type | `Count` |

## Runtime validation

Generated validators enforce schema bounds and wire encodings in addition to static types. Optional fields preserve absence; null is accepted only where declared. Wire data rejects cycles, functions, non-finite numbers, and unpaired Unicode surrogates.

Use the generated client/registration helpers with the matching SDK. Cancellation and deadlines are cooperative and do not reverse completed application effects.
