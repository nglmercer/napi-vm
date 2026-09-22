# Roadmap and implementation tracker

The executable specification lives in `tests/`. Each area below names the
suite that covers it, so a claim here can be checked without reading the
implementation.

## How to read this document

A feature is listed under exactly one status. The point of the taxonomy is
that a name existing in the global scope is not evidence that a feature works
— entries have appeared here as "completed" while being objects with no
members.

| Status | Meaning |
|--------|---------|
| **Full** | Behaves as specified for the cases the test suite covers. |
| **Partial** | Works for common shapes; documented gaps remain. |
| **Unsupported** | Not implemented. Reported as an error rather than silently mis-executed. |

Every claim below was checked against the current build.

## Full

### Language

- Arithmetic, operators, control flow (including labelled `break` and
  `continue`), functions, closures, recursion
- **Coercion**: `ToPrimitive` for `+` (a guest `valueOf` then `toString`), and
  the numeric conversions — `1 + [2]` is `"12"`, `[2] * 3` is `6`, `{} * 3` is
  `NaN` (`tests/errors.test.js`)
- **Lexical scoping**: block scope, `let`/`const`/`var` as distinct kinds,
  the temporal dead zone, `var`/function hoisting, per-iteration `let`
  bindings in `for` loops (`tests/scoping.test.js`)
- **Syntax errors**: malformed programs are rejected with a position instead
  of being partially executed; truncated input terminates rather than hanging
  (`tests/syntax-errors.test.js`)
- **Destructuring** in all three positions — declarations, assignments
  (`[a, b] = [b, a]`, `({ x } = o)`, property targets) and parameters,
  with defaults, nesting and rest elements (`tests/destructuring.test.js`)
- Spread (via the iterator protocol), rest, optional chaining, nullish
  coalescing, template literals
- Logical `&&`, `||`, and `??` short-circuit without evaluating an unused
  right operand (`tests/operators.test.js`)
- **Tagged templates**: the tag receives the cooked chunks, a `raw` companion
  array, and the interpolated values (`tests/builtin-constructors.test.js`)
- **Logical assignment**: `&&=`, `||=`, `??=`, with short-circuit evaluation
  (`tests/logical-assignment.test.js`)
- Numeric literals: decimal, hexadecimal, binary, octal, separators, and the
  `BigInt` suffix
- **Generators**: true suspension on a same-thread coroutine, `yield*`
  delegation, `throw()`/`return()`, iterator closing on early `for...of` exit
  (`tests/generators-delegation.test.js`, `tests/array-methods.test.js`)
- **Classes**: declarations *and expressions*, inheritance, `super(...)` and
  `super.method()`, static methods and fields, static initialization blocks,
  private fields and methods (`#x`), and `async`/generator methods
  (`tests/class-features.test.js`)
- **Object model**: property descriptors, `Object.create`, `defineProperty`,
  the `getOwnPropertyDescriptor(s)` pair, `getPrototypeOf`/`setPrototypeOf`,
  `hasOwn`, `fromEntries`, `is`, the integrity levels (`freeze`, `seal`,
  `preventExtensions`) and `Reflect`. `delete` removes the property, `in`
  walks the prototype chain, and `===` compares reference identity
  (`tests/object-model.test.js`)
- **Symbols**: a unique identity, `Symbol.for`'s registry, the well-known
  symbols, and symbol-keyed properties that stay out of `Object.keys` and
  `JSON.stringify` (`tests/symbols.test.js`)

### Asynchrony

- **Promises**: a real `Promise` constructor, pending promises, thenable
  assimilation, and `resolve`/`reject`/`all`/`allSettled`/`any`/`race`
- **A microtask queue**: reactions run as microtasks, so
  `Promise.resolve().then(f); g()` runs `g` first
- **Async functions** that genuinely suspend at `await`, including async
  arrows and async methods
- `for await…of`, async generators, and `Symbol.asyncIterator`
- `queueMicrotask`, and `setTimeout`/`setInterval`/`clearTimeout` on a
  clock-free timer queue: callbacks run after every microtask, ordered against
  each other by delay (`tests/promises.test.js`)

### Standard library

- `Map`, `Set`, `WeakMap`, `WeakSet`, keyed by SameValueZero
  (`tests/collections.test.js`)
- **Regular expressions**: literals and the constructor, `exec`/`test` with a
  stateful `lastIndex`, and `match`/`matchAll`/`search`/`replace`/
  `replaceAll`/`split` on strings. Character classes, greedy and lazy
  quantifiers, bounded repetition, alternation, capturing/non-capturing/named
  groups, backreferences, anchors, lookahead and lookbehind, and the
  `g`/`i`/`m`/`s`/`y`/`u` flags (`tests/regexp.test.js`)
- **BigInt**: arbitrary-precision integers with the arithmetic, bitwise and
  shift operators, `asIntN`/`asUintN`, and the type separation the language
  requires — mixed arithmetic is a `TypeError` (`tests/bigint.test.js`)
- **Typed arrays**: `ArrayBuffer`, all eleven views, and `DataView`
  (`tests/typed-arrays.test.js`)
- **`Date`** instances, with UTC accessors and ISO/JSON serialization
  (`tests/dates.test.js`)
- **`Proxy`** with the `get`, `set`, `has`, `deleteProperty`, `ownKeys`,
  `apply` and `construct` traps, and the `Function` constructor
  (`tests/proxy.test.js`)
- **Errors**: the built-in types, user-defined subclassing (`class E extends
  Error {}`), `stack` with the frames it was raised on, and `toString`.
  A derived class with no constructor gets the implicit
  `constructor(...args) { super(...args); }` (`tests/errors.test.js`)
- `TextEncoder`, `TextDecoder`, `URLSearchParams` and `structuredClone` —
  the web globals that are pure computation (`tests/web-globals.test.js`)
- The built-in namespaces are callable: `String(x)`, `Number(x)`,
  `Boolean(x)`, `Object(x)`, `Array(n)`, plus `Array.from`/`of` and
  `String.raw` (`tests/builtin-constructors.test.js`)

### Modules

- Named, default and namespace imports and exports, with **live bindings**:
  an imported name tracks the exporting module
- Re-exports (`export … from`), `export *`, `export * as ns`
- `import(specifier)`, resolving to a namespace object
- Cyclic graphs, through `defineModule` (which records a module without
  evaluating it, so the bodies run on first import). A forward reference binds
  the cell the exporting module will fill, which is what the specification's
  separate link and evaluate phases achieve.
- **Per-module scope**: a module's declarations belong to the module, so two
  modules can each declare `helper` and neither leaks to the global object
- Namespace objects expose the default export as `"default"`
  (`tests/modules-linking.test.js`)
- `Vm.validateModule()` lexes and parses source without resolving imports or
  executing side effects (`tests/vm-validation.test.js`)

### Host integration

- Crash-safety guards: recursion, parse depth, loop budget, array/string
  caps, generator nesting, regular-expression backtracking, and the job queue
  (`docs/safety.md`)
- **N-API value bridge**: structured data, `Date`, `BigInt`, symbols,
  `ArrayBuffer` and typed arrays, `Map`/`Set`, settled promises, and cyclic or
  shared references, in both directions. A VM **function** crosses as a host
  callable that re-enters the interpreter, keeping its closure
  (`tests/bridge-values.test.js`)
- Plugin capability host: manifests, permissions, and the capability APIs
  — `napi:fs`, `napi:path`, `napi:crypto`, `napi:timers` and standard `fetch()`,
  each installed only when the manifest asks *and* the host policy permits
  (`tests/plugins/capabilities.test.ts`, `docs/plugins.md`)
- **LSP**: synchronization, completion, hover, document symbols, definition,
  references, document highlight, rename, signature help, inlay hints,
  semantic tokens, formatting and code actions (`tests/lsp_protocol.rs`)

## Partial

- **npm guest packages** — `GuestPackageLoader` resolves ESM `exports`,
  subpaths, relative modules and dependencies into canonical guest IDs, then
  registers source with `defineModule`. The verified Valibot 1.5.0 fixture
  matches Node and Bun for primitives, nested schemas, variants and a
  transformation pipeline. CommonJS/native packages and computed dynamic
  imports are not supported yet; compiler mode defaults to `none`
  (`tests/npm-loader.test.js`, `tests/compat/valibot.test.js`)

- **Generators on `wasm32`** — the browser target has no stack switching, so a
  body cannot be suspended. It runs once to completion on the first `next()`
  and its yields are buffered for the remaining calls to drain. Values,
  `for…of`, spread, `Array.from` and `yield*` all work
  (`tests/wasm/browser-build.test.mjs`), but the difference is observable: the
  body's side effects happen at the first `next()` rather than interleaved
  with the consumer, `next(v)` cannot send a value in, abandoning a `for…of`
  early does not stop a body that has already run, and an unbounded generator
  hits a cap and raises a catchable `RangeError`.

  Real suspension needs one of three things, none of them small:

  - **A resumable evaluator** — `eval_expr`/`eval_stmt` rewritten to run on an
    explicit continuation stack instead of the native one, so a `yield` can
    return to the driver mid-expression. This is the general answer and works
    for every body, but it is a rewrite of the interpreter core.
  - **A CPS transform of generator bodies** — compile a body containing
    `yield` into a state machine. Narrower in blast radius, but the subtle
    cases (a `yield` inside a loop condition, inside `try`/`finally`, inside a
    closure's default argument) are where such transforms go wrong.
  - **Asyncify** — binaryen's stack-switching transform over the built module.
    Cheapest to implement, but it is a build-pipeline dependency this repo
    deliberately does not have (see the `wasm-opt` note in `Cargo.toml`), and
    it costs module size and speed on every path, not just generators.

  A *partial* transform — handling the recognizable shapes and buffering the
  rest — is worse than the current behaviour, which is at least uniform and
  documented. It would make a browser generator's semantics depend on whether
  its body matched a pattern.
- **N-API boundary** — an exported VM function cannot be called *while the VM
  is already running*: the interpreter is single-threaded, so a host callback
  that fires from inside a VM execution is refused with "VM is busy" rather
  than running two executions at once. Generators cross as `undefined`, since
  a host iterator would need the same re-entrancy.
- **LSP formatting** — indentation only. A formatter that reconstructed source
  from the AST would delete every comment, since the parser does not keep them,
  so this one works on the text and changes exactly two things: each line's
  indentation, and trailing whitespace. It never moves a newline — automatic
  semicolon insertion depends on where they are — which is also why the output
  is less tidy than a full pretty-printer's.

## Unsupported

Reported as errors rather than silently mis-executed:

- Network operations remain unavailable unless the host grants them. Standard
  `fetch()` is installed only through the fetch capability;
  `crypto` and `timers` continue to use their explicit capability APIs. Every
  request is checked against the manifest and host policy before network I/O.
- `WebSocket` is still a non-callable placeholder. The runtime does not yet
  have a thread-safe external-event queue for delivering host messages at a
  safe interpreter checkpoint, so there is no WebSocket transport installed.
- `Intl`, `Object.groupBy`, and the other recent library additions not listed
  above.
- `with`, which is not in the grammar at all. (Labelled `break` and `continue`
  *are* supported, including on a labelled block.)

## Priority order

1. A resumable evaluator, for true generator suspension on `wasm32` — the one
   remaining Partial that is a missing capability rather than a stated
   boundary. See the entry above for the three ways in and what each costs.
2. Re-entrant host calls, so a VM function exported to the host can be called
   from inside a VM execution instead of being refused as busy. Generators at
   the boundary need the same thing.
3. A pretty-printer that can rewrap lines, which needs the parser to retain
   comments. Today's formatter only re-indents.
4. `Intl` and the other recent library additions, if a consumer asks for them.
   None are load-bearing for the sandbox.

## Known boundaries

- The interpreter is not a replacement for a full JavaScript engine.
- `runAsync` creates one OS thread per invocation and is not intended for
  high-frequency events. Call `dispose()` when finished with such a VM.
- The in-process sandbox needs worker/process isolation for strict untrusted-code
  CPU and memory limits.
- Host functions are explicitly typed by metadata; JavaScript runtime functions
  do not carry TypeScript annotations.
- A module scope chains to the VM's persistent global, so a module can read
  what a script declared through `run`. That is this VM's model — `run` is a
  REPL-like persistent global — and it runs one way: script scope does not see
  a module's declarations.
- String and array indices count Unicode scalar values, not UTF-16 code units,
  so text outside the Basic Multilingual Plane is indexed differently from a
  real engine.

Contributions should add or update a regression test with each language or
bridge feature, then run the quality gate from `docs/development.md`.
