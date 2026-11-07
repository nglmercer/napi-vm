# Implementation map and remaining work

This page is a guide to the checked-in implementation, not a completeness
claim or a release schedule. The tests and source linked below are the
starting point for checking a particular behavior. Historical plans and
measurements describe the revision and environment in which they were made.

## Engine

The engine has a lexer, parser, interpreter, bytecode compiler and verifier;
unsupported compilation cases can use the AST evaluator. Rust, Node and WASM
entry points share the core implementation. See [crate architecture](crate-architecture.md),
[development](development.md), and the tests in [`tests/`](../tests/).
Strings use UTF-16 code units; implementation and migration evidence are in
[UTF-16 validation](utf16-extraction-validation.md).

The Test262 runner and pinned corpus provide a measured compatibility target,
not a guarantee about all JavaScript programs. The original corpus and the
separately labelled `reviewed-5112` overlay have different expectations. Use
[Test262 status](test262-full-pass-plan.md) and its linked reports for revision,
selection, timeout and result details; do not combine their scores.

## Runtime and host boundaries

The default engine does not install ambient filesystem, network, console or
timer globals. Optional runtime features and explicit configuration supply
host facilities; Cargo features compile code but do not by themselves grant
permissions. See [optional runtime](optional-runtime.md),
[event-loop scheduling](event-loop.md), [runtime ownership](runtime-improvements.md),
and [safety](safety.md).

Native addon support has separate Node sidecar and in-process backends with
different capabilities and platform constraints. See [native addon embedding](native-addons.md)
and [the native addon example](../examples/rust_native_addon.rs). The older
[native addon loader plan](native-addon-loader-plan.md) and
[Node addon runtime plan](node-addon-runtime-plan.md) are design history,
not current compatibility matrices.

The capability-based [plugin host](plugins.md) is separate from the trusted
process plugin SDK and protocol in [`docs/trusted-plugins/`](trusted-plugins/).
The [editor guide](editor.md) describes LSP and runtime metadata integration;
the [runtime protocol](runtime-protocol.md) specifies their wire format.

## Remaining work

Priorities depend on the embedding and target. Check an area's existing tests,
open issues and recent evidence before choosing work. In particular:

- Maintain conformance and performance measurements as the engine changes;
  keep corpus provenance and timeouts with the results.
- Validate behavior on each supported native and WASM target, especially
  where coroutine, operating-system or native-addon behavior differs.
- Expand runtime and host APIs only with explicit permissions, bounded resource
  ownership and regression coverage; presence of an API does not establish
  complete Node or browser compatibility.
- Keep editor and package behavior aligned with the shipped interfaces.

The old [next implementation sequence](next-implementations.md),
[next steps](next-steps.md), [runtime plan](runtime-plan.md), and
[native addon runtime plan](node-addon-runtime-plan.md) contain historical
context. Use the implementation and current tests before acting on an old
checklist. Past validation reports and checkpoints are snapshots, not current
status; the [documentation index](README.md) distinguishes them from guides.
