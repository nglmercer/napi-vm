# Next implementation sequence

This plan continues the optional runtime implementation in PR #18. It describes
future work; checked-in APIs and tests are documented in
[optional-runtime.md](optional-runtime.md). Completing a stage requires its
acceptance checks, not just adding feature names or globals.

Current branch evidence: 606 workspace all-features Rust tests passed, four
ignored; standalone core, minimal runtime and facade tests also passed. The historical
Test262 baseline is 32,359 / 102,956 variants (31.43%). It must be rerun against
the current engine before using it to judge progress. Web, Node and npm corpus
coverage is not yet measured. Runtime preview and stable-engine gates remain open.

## Order and dependencies

| Stage | Priority | Deliverable | Depends on |
| --- | --- | --- | --- |
| 1 | P0 | Reliable conformance runner and current baseline | PR #18 |
| 2 | P0 | End-to-end lossless UTF-16 | Stage 1 |
| 3 | P0 | Separate core/runtime crates and feature CI | Stage 2 |
| 4 | P0 | Module linking, async execution, bytecode and GC correctness | Stages 1–3 |
| 5 | P0 | Runtime cancellation, permissions and resource accounting | Stage 3 |
| 6 | P1 / preview | Reproducible npm graphs and ESM/CJS interop | Stages 4–5 |
| 7 | Preview | Measured runtime preview | Stages 1–6 |
| 8 | P1 | Node compatibility tiers and permissioned addons | Stages 3–6 |
| 9 | P1 | TypeScript checking and mapped stack traces | Stages 3–4 |
| 10 | P2 | Remaining CLI commands and platform support | Stages 5–9 as needed |

The UTF-16 migration and core/runtime extraction are implemented on the current
branch. The following stages remain acceptance checklists and historical scopes.
The next implementation should close engine execution and reachability gaps:
runner accuracy and the remaining module/execution semantics are preview blockers.
After crate extraction, Node and TypeScript work can proceed independently of
preview validation. Preview does not require completing every Node tier.

## 1. Make engine compatibility evidence trustworthy

Proposed PRs:

1. Separate parse, resolution/link and runtime error phases in the Test262
   worker; load complete fixture graphs using the module loader.
2. Add realm, buffer-detachment and GC host hooks, then agent support behind
   an explicit test-only capability. Unsupported hooks remain visible as
   harness errors rather than successes or silently excluded tests.
3. Publish a new full pinned baseline and a failure inventory grouped by
   feature, error phase and execution mode.

Acceptance:

- Worker contract tests cover strict/non-strict, raw, module, async completion,
  include failures and every negative phase; crashes and timeouts are distinct.
- Reports include corpus revision, worker digest, runtime limits, selected
  variants, exclusions and denominator. Archive CI artifacts for comparison.
- Run a small deterministic selection on each engine PR and the full pinned
  corpus on scheduled CI. Baseline changes require explained differences.
- Choose subsequent semantic fixes from failure clusters; do not set an
  unsupported percentage target for an individual PR.

## 2. Implement UTF-16 as one coherent engine migration

Implemented on the current branch with code-unit strings throughout both
execution paths and explicit host conversions. Expand conformance coverage
against the acceptance checklist below before making a stable engine claim.

Implementation slices on that branch:

1. Introduce an immutable code-unit string type preserving all u16 sequences.
   Define equality, ordering, hashing, checked length, concatenation, slicing,
   code-point iteration, and explicit host UTF-8 conversion behavior.
2. Migrate lexer escapes, template cooked/raw strings, AST literals and
   bytecode constants. Reject invalid escapes with syntax errors; preserve lone
   surrogates in valid escapes.
3. Migrate ToPrimitive/ToString, boxed strings, symbols, string methods,
   property names and enumeration. Object keys must round-trip without
   collisions, including any internal encoding marker.
4. Implement JSON parse/stringify and regex code-unit offsets. Distinguish
   non-Unicode regex matching from Unicode code-point matching.
5. Audit N-API UTF-16 creation/extraction, wasm/JS conversion, module IDs,
   error messages and transport boundaries. Define replacement or rejection
   where a host protocol requires valid Unicode; never silently change engine
   strings before that boundary.

Acceptance:

- AST and bytecode agree on BMP, astral, lone high/low surrogate, reversed pair,
  embedded NUL and mixed inputs. No unsupported case silently substitutes U+FFFD.
- Cover length/indexing, charAt/charCodeAt/codePointAt, slice/substring, search,
  split, padding, concatenation, iteration, JSON and regex offsets.
- Surrogate-containing property keys survive assignment, lookup, deletion,
  Object.keys, Reflect.ownKeys and JSON round-trips without collisions.
- Native and wasm bridges round-trip code units; host UTF-8 conversion has
  explicit tests for its chosen behavior.
- Relevant pinned Test262 selections pass and the full corpus has no unexplained
  regressions. Rerun core/default/all-features/wasm checks.

## 3. Extract the crate boundary

Proposed PRs:

1. Move parser, values, AST/bytecode, ECMAScript builtins, Promise jobs, engine
   limits and loader contracts into napi-vm-core with one interpreter owner.
2. Move runtime builder, OS scheduling adapters, transports, permissions,
   packages and Web/Node wrappers into napi-vm-runtime, depending on core.
3. Keep napi-vm as the compatibility facade for bindings and CLI. Document the
   EngineBuilder rename and explicit runtime installation for existing users.

Acceptance: compile core independently; audit its dependency tree for runtime
transports and OS capabilities; retain default = ["napi"] in the facade; test
no-default/default, each runtime feature, all-features and wasm in CI. Embedding
core must not install runtime globals or OS-backed loaders. Run existing Rust,
Node and browser adapter tests with explicit host capabilities.

## 4. Close engine execution and reachability gaps

Use separate PRs for module linking, async semantics, bytecode coverage and GC:

- Implement module graph instantiation separately from evaluation, including
  live bindings, cycles, re-exports, dynamic import and top-level await where
  supported. Match linking failures to the correct Test262 phase.
- Validate Promise job ordering, thenable assimilation, async/await rejection
  propagation, iterator closing and suspended execution roots.
- Use AST/bytecode differential tests for newly compiled constructs. Keep
  fallback for declined constructs and expose fallback reasons to test tooling.
- Audit roots for closures, generators, modules, host handles, pending IO,
  WeakMap/WeakSet ephemerons, WeakRef and FinalizationRegistry cleanup jobs.

Acceptance: targeted Test262 groups and differential tests pass; deterministic
forced-collection tests prove required reachability and eventual eligible
cleanup without promising GC timing. Full corpus deltas identify remaining
semantic failures. A stable engine claim still requires at least 90% Test262;
95% remains the long-term target.

## 5. Harden the existing runtime

Proposed PRs:

1. Make every socket connection/handshake/read/write and HTTP server operation
   cancellable with bounded teardown. Test abort, drop, full queues, late
   completions, reconnect errors and resource release under load.
2. Centralize capability checks across redirects, imports, package cache,
   Node IO and addons. Test host/port changes, credential handling, path races,
   symlinks, hardlinks, malformed grants and permission merging.
3. Define heap accounting and limits for VM allocations, strings, buffers,
   module caches and queued jobs. Enforce budgets before allocation where
   possible; distinguish heap limits from Unix process address-space limits.
4. Improve Web semantics: EventTarget/abort dispatch, Request/Response body
   consumption and cloning, full URL APIs, streaming bodies, Blob/FormData,
   and WebCrypto algorithms in separately testable subsets.

Acceptance: workers never receive VM values or enter the interpreter; limits
and cancellation work during pending IO; adversarial guest tests fail closed.
Pin relevant WPT subsets and report passes, failures and unsupported cases.
Document the sequential HTTP/1.x server limits until chunking, keepalive and
server TLS are implemented and tested.

## 6. Complete reproducible packages

Proposed PRs:

1. Complete npm semver, aliases, exports/imports error behavior and conditions;
   cover package type, self-reference and subpaths for ESM and CommonJS.
2. Add peer/optional dependency policy, deterministic graph resolution and
   complete locked replay. Introduce transactional staging and rollback for
   upgrades and interrupted installs; validate cache metadata and bytes.
3. Complete ESM/CJS namespace behavior and cyclic interop. Build a pinned real
   package corpus with separate install, import and behavior outcomes.

Acceptance: locked graphs reproduce without resolution changes; permitted
cache-only replay works without unintended network access; tampered archives,
malicious paths and denied imports fail before evaluation. Failed installations
leave the previous graph usable. Document unsupported package shapes and retain
an explicit policy for lifecycle scripts.

## 7. Validate runtime preview

Release a preview only when every required item has executable evidence:

- Engine: UTF-16, Test262 runner, bytecode with fallback, modules, Promises and
  async/await.
- Runtime: event loop, external queue, timers, filesystem, fetch, WebSocket,
  TCP and denied-by-default permissions.
- Packages: ESM, CommonJS, package.json and npm resolution.
- CLI: run, eval and repl.

Run CLI and embedding scenarios from clean installations. Publish separate
Test262, WPT, Node and npm reports with pinned revisions; do not combine scores.
Preview does not imply meeting the stable-engine 90% gate. Do not announce
preview while UTF-16 or required package/runtime scenarios remain incomplete.

## 8. Expand Node compatibility by tier

- Finish Tier 1: node:url, process and os, and remaining path/util/events/assert/
  buffer behavior. Environment and system information need explicit policy.
- Tier 2: fs, stream, crypto, http and https, adapting the existing runtime
  transports with operation-level permissions and backpressure.
- Tier 3: net, tls, dns, zlib, child_process and worker_threads. Specify worker
  isolation, transferred buffers, shared memory, lifecycle and resource budgets.
- Wire runtime-native-addons to a provider with explicit --allow-ffi and any
  required process grant. Validate addon allowlists/integrity; document the
  isolation guarantees for native code.

Acceptance per tier: pinned Node tests, denied/granted capability tests, shutdown
and resource limits, ESM/CJS loading and real-package cases. Report partial
modules explicitly rather than exposing inert placeholders.

## 9. Finish TypeScript developer behavior

First integrate generated source maps with guest stack frames, including imported
modules, async calls and transformed TSX. Then implement a real type checker for
`check`; Oxc erasure alone cannot supply diagnostics. Choose and document the
checker integration, version pinning, dependency policy and offline behavior.

Acceptance: errors point to original file/line/column; maps are retained through
loader caching; check detects semantic type errors and exits nonzero; run can
transpile without invoking check. tsconfig, JSX options and declaration-file
handling have documented supported subsets.

## 10. Complete CLI and platform work

Add test/bench/task first, then fmt/lint and executable installation. Keep these
as separate PRs with command help, exit-code contracts and integration tests.
Extend check through Stage 9. Subprocess tasks and executables require explicit
process policy; execution inherits resource budgets.

Implement secure Windows file access and platform resource supervision before
advertising equivalent guarantees there. Keep unsupported behavior explicit.
Add Linux/macOS/Windows CI and wasm embedding checks for the relevant capabilities.

## Review and release rule

Every implementation PR describes supported behavior, remaining limitations and
validation. Update optional-runtime.md and this sequence when a gate is met.
Prefer bounded PRs; the UTF-16 migration is the exception because publishing
mixed string representations would break correctness. No checklist item moves
to complete solely because compilation or a targeted smoke test passes.
