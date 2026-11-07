# example.greeter

Generated contract reference. Regenerate this page with its TypeScript, Rust, and JSON artifacts; do not edit it by hand.

| Identity | Value |
| --- | --- |
| Interface version | `1.0.0` |
| Contract digest | `ca8aa1104513e5324f7aaaca699f3c4160f4b35ba934b4c1fc3d236aa77c9824` |
| Descriptor version | 1 |

An asynchronous interface exchanging validated data-only messages.

Host and plugin must agree on the exact interface version and digest. A package version or matching method name alone is insufficient.

## Methods

### `greet`

| Boundary | Type |
| --- | --- |
| Input | `GreetInput` |
| Resolved output | `GreetOutput` |

The JavaScript client returns a promise; the Rust client is asynchronous. Declared domain errors: `INVALID_NAME`.

Idempotent metadata: `true`. Peers do not automatically retry calls.

## Data types

### `GreetInput`

```typescript
type GreetInput = { "name": string };
```

### `GreetOutput`

```typescript
type GreetOutput = { "message": string };
```

### `InvalidNameData`

```typescript
type InvalidNameData = { "reason": string };
```

## Domain errors

### `INVALID_NAME`

A declared application error.

Data type: `InvalidNameData`.

## Events

No events are declared.

## Runtime validation

Generated validators enforce schema bounds and wire encodings in addition to static types. Optional fields preserve absence; null is accepted only where declared. Wire data rejects cycles, functions, non-finite numbers, and unpaired Unicode surrogates.

Use the generated client/registration helpers with the matching SDK. Cancellation and deadlines are cooperative and do not reverse completed application effects.
