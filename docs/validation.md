# Validation and compatibility

Compatibility is established for an input corpus, feature set, binary,
platform, and boundary. The project's ECMAScript results, Node bindings,
optional runtime, native-addon backends, and trusted-plugin interoperability
have separate checks.

## ECMAScript conformance

The pinned upstream Test262 revision is recorded in
[corpus.lock.json](../tools/test262/corpus.lock.json). The original corpus is
available unchanged. The `reviewed-5112` profile overlays one reviewed correction
to `annexB/language/function-code/block-decl-func-skip-arguments.js` from the
identified upstream PR #5112; it is labelled separately because it is not the
unmodified upstream corpus.

The retained reviewed-profile run covers **53,615 files / 102,992 variants**,
all passing, with zero skips, failures, timeouts, crashes, or harness errors.
It used two workers and a ten-second deadline. The
[conformance evidence index](../tools/test262/evidence/reviewed-5112-full-2026-11-06/README.md)
links the full compressed report, counts, comparison, and audit. The subsequent
[performance validation](../bench/baselines/performance-review-2026-11-06/validation.json)
retains another complete run against the optimized engine and the same variant
set. Each report identifies its own engine digest; do not substitute an arbitrary
local executable with the same filename.

The [runner guide](../tools/test262/README.md) explains fetching, smoke runs,
complete gates, retesting, checkpoints, and comparisons. A smoke selection
cannot prove that every previously passing variant still passes.

## Host and platform checks

| Boundary | Evidence to use |
| --- | --- |
| Core values, properties, parser, bytecode and AST fallback | Rust unit/integration tests and complete selected Test262 run |
| Native Node conversion, loaders, async exit | [Node suite](../tests/node/) against a freshly built addon |
| Browser-target binding behavior | [WASM suite](../tests/wasm/) after rebuilding `playground/pkg` |
| LSP/runtime metadata | LSP Rust tests and `runtime/smoke.cjs` |
| Optional runtime permissions/I/O | [Runtime crate tests](../crates/napi-vm-runtime/tests/) with selected features |
| In-process Node-API | Native fixtures, differential tests, callbacks/finalizers, platform CI |
| Trusted plugins | Protocol, SDK, host, contract, package, and cross-language suites |

The main workflow tests Node 22, 24, and 26 using a built addon, alongside
native/WASM/LSP and platform-specific checks. Other workflow files cover
runtime and trusted-plugin matrices. A workflow definition states what CI is
configured to do; inspect actual job reports before claiming a target passed.

## Known compatibility boundaries

The engine is not Node.js, V8, a browser DOM, or tsserver. Optional Node/web
APIs cover implemented subsets. Packages can require unsupported host services,
syntax transformations, native imports, or platform assumptions.

Guest strings preserve lone UTF-16 surrogates. The trusted-plugin wire format
requires Unicode-scalar JSON strings and rejects unpaired surrogates. Native
bridge conversion supports more than JSON but does not guarantee arbitrary
structured-clone identity. These boundaries are intentional and must be
considered when moving an application between integrations.

Use [development](development.md) for current commands and [safety](safety.md)
for limits. Passing conformance or a faster benchmark does not establish a
security guarantee or compatibility outside the tested boundary.
