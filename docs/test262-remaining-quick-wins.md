# Remaining Test262 quick wins

Base: merged PR #26, `142a38d`. Corpus:
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`. Rust 1.99.0, Linux x86_64.

## Scope and audit

This applies the original quick-win audit across Object, Reflect, Array, String,
Number, Math, Date, JSON, Map, and Set. It does not claim complete ECMAScript
conformance or implement the broader subsystems excluded by the original plan.
The corpus, runner assertions, resource limits, and dependencies are unchanged.

The merged engine was rerun over all ten groups (19,689 variants) and the global
numeric parsers (218 variants). Failures were triaged using the existing tool.
The full baseline is the preceding PR's complete after report: its exact worker
SHA-256 matches the freshly exercised merged baseline worker. That evidence is
reused rather than treating the older `latest.json` count as the current base.

| Original audit area | Result |
| --- | --- |
| Object.is, hasOwn, getOwnPropertyDescriptors, hasOwnProperty | Existing selected tests already pass; implementation retained |
| propertyIsEnumerable | Reuse ToPropertyKey and own-descriptor operations, including symbol conversion and correct exception order |
| Reflect.has / ownKeys | Existing tests pass; implementation retained |
| Other Reflect metadata / receiver checks | Correct get.length, namespace tag, and getPrototypeOf primitive rejection; all Reflect variants pass |
| Array.at | Generic receiver, ordered length/index conversion, inherited holes and indexed Get |
| Array.with | Ordinary dense copy, negative indexes, range checks, no species lookup or replaced-index Get |
| Array.includes / findLast / findLastIndex | Correct lengths/metadata and safe-integer logical lengths without allocating huge arrays |
| String.at / padStart / padEnd | Correct metadata and shared receiver ToString with observable conversion order |
| String.isWellFormed / toWellFormed | UTF-16 inspection and replacement of unpaired surrogates only |
| Number predicates | Existing numeric classification retained; fix method lengths and descriptors |
| Math methods from the original list | Already pass after PR #26; implementation retained |
| Number constructor / prototype | Shared ToNumeric, BigInt conversion, receiver validation, argument conversion/ranges, constructor and method metadata |
| Number.toFixed | Exact binary64 scaling using existing integer operations; correct tie rule, signed zero, NaN/Infinity, and large values |
| parseInt / parseFloat | Observable ToString hooks and existing ECMAScript whitespace trimming; global/static identity retained |
| Date | Constructor metadata/default primitive hint, timezone receiver checks, standard non-locale formats, signed-year and own-output parsing, toPrimitive descriptor |
| JSON | Metadata/tag, ToString for parse input, SyntaxError for malformed JSON, strict numeric grammar, and duplicate-key replacement |
| Map / Set | Species getters, branded size accessors, and callback validation even for empty collections |

Allocation caps are kept separate from logical array-like lengths for the read
operations that need safe-integer indexes. Their loops charge the existing loop
budget, including AST execution; a new regression sets a three-iteration budget
and verifies early termination. No loop or allocation limit was disabled.

## Focused measurement

| Group | Variants | Before passes | After passes | New passes | Lost passes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Object | 6,802 | 6,547 | 6,565 | 18 | 0 |
| Reflect | 306 | 298 | 306 | 8 | 0 |
| Array | 6,117 | 5,115 | 5,241 | 126 | 0 |
| String | 2,443 | 1,901 | 2,141 | 240 | 0 |
| Number | 680 | 522 | 648 | 126 | 0 |
| Math | 654 | 470 | 470 | 0 | 0 |
| Date | 1,188 | 1,082 | 1,148 | 66 | 0 |
| JSON | 330 | 200 | 264 | 64 | 0 |
| Map | 405 | 357 | 371 | 14 | 0 |
| Set | 764 | 446 | 454 | 8 | 0 |
| Global parseInt / parseFloat | 218 | 198 | 214 | 16 | 0 |

Combined: **686 new passes, zero lost passes** across 19,907 variants. No skips,
timeouts, crashes, or harness errors occur in these focused runs.

## Full-corpus measurement

| Corpus | Variants | Before passes | After passes | New passes | Lost passes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Complete pinned Test262 | 102,956 | 75,637 | 76,411 | 774 | 0 |

Full pass rate increases from 73.4654% to **74.2171%**. There are exactly 774
status transitions. Both reports have the same two deep-WeakMap staging timeouts,
with zero skips, crashes, and harness errors. This gain is additional to PR #26.
The final worker hash is `c40aac3ada611ce636becb5bf169ad10d58cc297ca5f01119d84feae5584035e`.

The raw comparison has 774 pass transitions. Two Atomics FIFO script variants
pass in this full run but fail again in repeated focused runs on both workers;
they are scheduling-sensitive and are **not credited as reproducible fixes**.
The other **772 improvements** are replayed on both workers using every affected
file and its normal variants. Focused replay evidence is archived separately;
none of it replaces full-run outcomes. That replay has **zero passes before,
772 passes after, and zero failures after**, using the same final worker digest. The reproducible gains outside the broad
focused selections total 86. The native-search loop budget is also covered by a
Rust regression in both tiers.

All 33 specifically audited method subgroups in candidate-audit.json have zero
remaining nonpassing variants. This does not describe the entire groups outside
those audited methods.

## Validation

Final root suite: **329 pass, five existing failures**. Full core suite: **639
pass, three existing failures**. All 22 new regressions and the retained Math and
Date regressions pass.

There are 22 new regression tests. Each asserts verified bytecode selection and
runs the same script separately as an AST. They cover metadata/descriptors,
receiver and argument validation, side effects and exception order, sparse and
inherited elements, safe-integer lengths and budgets, UTF-16, exact number
rounding, JSON syntax/duplicate keys, species, and Date formatting/round trips.
The seven previous Math regressions are retained.

One existing Date assertion previously expected ISO text from the default
primitive hint. Its exact expected string is corrected to the standard
Date.toString format. The exact-equality assertion and both execution paths are
retained; no assertion is removed or weakened.

The broader repository is not entirely green:

- Root Rust suite has five existing failures: the cache-count assertion in
  evaluation_modes, the object-rest bytecode expectation in object_spread, and
  three LSP tests where this environment rejects Unix socket binds.
- Full core testing with `--no-fail-fast` reveals three existing failures: the
  shape transition lookup, named-function-expression strict assignment in
  grammar_contexts, and implicit-global rejection in module_execution.
  All three reproduce on an isolated unchanged `142a38d` checkout (639 pass,
  three fail), with no shared target directory.
- Standard targeted Clippy passes with the same six warnings in untouched code.
  Strict `-D warnings` remains blocked by those known warnings. No suppressions
  or unrelated cleanup are included.
- N-API plus WASM feature compilation is checked on the host target; this is not
  a claim of wasm32 execution coverage.
- All 12 Test262 runner/tooling unit tests pass.

Full-corpus parity between AST and bytecode is not claimed: the Test262 worker
uses its existing evaluation path. The new Rust regressions explicitly assert
both paths for every implementation area.

## Reproduce

Build a worker on the unchanged base and preserve it before building this branch:

```sh
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path built-ins/Object --path built-ins/Reflect \
  --path built-ins/Array --path built-ins/String \
  --path built-ins/Number --path built-ins/Math \
  --path built-ins/Date --path built-ins/JSON \
  --path built-ins/Map --path built-ins/Set \
  --output artifacts/test262/remaining-validated.json
```

Run parseInt/parseFloat separately with `--path built-ins/parseInt --path
built-ins/parseFloat`. For the full corpus omit all `--path` arguments. Use the
same jobs/timeout/corpus settings on both workers, then:

```sh
python3 tools/test262/compare.py BEFORE.json AFTER.json --output transitions.json
python3 tools/test262/triage.py AFTER.json --output triage.json
cargo test --locked --no-default-features --test builtin_quick_wins
cargo test --locked --no-default-features --test math_quick_wins --test date_setters
cargo test --locked --no-default-features --no-fail-fast
cargo test --locked -p napi-vm-core --no-default-features --no-fail-fast
cargo fmt --check
cargo clippy --locked --no-default-features \
  --lib --bin napi-vm-test262 --test builtin_quick_wins
cargo check --locked --features wasm
```

Compressed reports retain every selected outcome and worker identity. The
implementation patch and its hash identify the exact source on top of the base.
The baseline full report remains in `evidence/math-quick-wins/full-after.json.gz`;
follow-up evidence is under `tools/test262/evidence/remaining-quick-wins/`.

## Deferred work

All specifically named Object/Reflect/Array/String methods in the original audit
pass their selected groups after this batch. Number predicates and the named
Math methods also pass. Remaining failures are reported rather than skipped:

- Date locale-aware formatting needs a consistent locale/Intl implementation;
  toTemporalInstant needs Temporal. Installing placeholder methods for metadata
  tests alone would not implement their behavior.
- JSON revivers require recursive property traversal, replacement/deletion and
  observable descriptor behavior; rawJSON requires its own validated raw-value
  representation. Neither is a trivial error-type or metadata fix.
- Live Map/Set iteration and forEach mutation semantics need storage/iterator
  changes that preserve insertion order, deletion, re-addition and additions
  during iteration. Snapshot iteration is retained, with failures visible.
- Set composition and Map.groupBy are additional features outside the original
  simple collection-validation audit.
- Number.toExponential and remaining toPrecision behavior need separately
  verified formatting algorithms. Other Math methods beyond the original list
  remain a separate batch with accuracy/edge-case testing.
- Other Array copying/sorting methods, locale-sensitive String operations,
  normalization, and advanced RegExp behavior remain outside this batch.
- No Proxy, GC, realm, module-system, interpreter, or bytecode redesign is made.
