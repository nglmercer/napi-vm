# Unicode, Annex B, and built-in conformance continuation

The target is at least 97,809 passes out of all 102,956 variants at Test262
revision `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`. Focused selections do not
establish completion.

## Implementation

- String casing uses ICU context-sensitive Unicode mappings. Locale casing
  validates the observable locale list and supports language-sensitive rules.
  Normalization supports NFC, NFD, NFKC, and NFKD. Unpaired UTF-16 surrogates
  survive both transformations. Primitive string property lookup respects the
  intrinsic prototype, and string search/split coercions retain exception order.
- Function declarations instantiate once at scope entry. Reaching a declaration
  preserves function identity, assigned properties, and prior reassignment.
  Annex B block functions get eligible variable bindings at function/script/eval
  entry and update them when their declaration executes. Parameters, enclosing
  lexicals, catch patterns, loop bindings, strict code, and global declaration
  restrictions participate in eligibility. Both bytecode and the AST evaluator
  implement these rules; bytecode has verified AnnexBInit/AnnexBUpdate operations.
- Intl.Collator uses ICU UTF-16 collation with sensitivity, punctuation, numeric,
  and case options, a cached bound compare function, and resolvedOptions.
  String.localeCompare uses the same implementation. The cached compare function
  is traced and cleared by the cycle collector.
- Intl.PluralRules uses ICU cardinal/ordinal and plural-range rules. Decimal
  rounding uses fixed_decimal before selecting a category; digit precision,
  increments, rounding mode/priority, and trailing-zero options are observable.
  Compact notation supplies a significand and exponent to plural operands.
- Dynamic import evaluates specifier/options before reporting errors through its
  promise. JavaScript source-phase imports reject with SyntaxError after loading.
  Module namespaces have a non-writable, non-enumerable, non-configurable
  Symbol.toStringTag and apply ordinary descriptor compatibility to symbol keys.
- Error.prototype.stack has branded accessors and internal captured-stack data.
  The setter validates strings and performs the receiver's own descriptor,
  creation, or Set operation, preserving Proxy traps and abrupt completions.
- RegExp.escape quotes leading ASCII letters/digits, syntax, punctuation,
  whitespace, line terminators, and isolated surrogates without splitting pairs.
- BigInt wrapping applies ToIndex then ToBigInt. Widths that already contain the
  input return it without allocating proportional to width. BigInt.toString
  supports radix conversion and observable radix coercion. Symbol conversion,
  registry arguments, constructor/method descriptors, and primitive prototypes
  follow their ECMAScript operations. Increment/decrement preserve numeric type
  and return the converted previous value in both execution tiers.

The prior committed batch also added immutable buffers and Uint8Array codecs.
Its 140 selected Uint8Array variants pass, with no corpus or harness changes.

## Validation and limitations

The regression tests cover function identity and Annex B eligibility, Unicode
surrogate preservation and casing context, codec prefix writes, bound Collator
collection, plural fraction/ordinal rules, Error stack Proxy operations, and
BigInt conversion order.

The complete core library run has 355 passes and one existing failure in
`interpreter::node_addon::tests::loads_and_invokes_a_real_node_api_addon`, which
reports `Cannot redefine property: prototype` in its native fixture. All
bytecode compiler and parity checks pass. The two compiler tests that explicitly
expected duplicate hoisted function objects now require one instance; tier
requirements were preserved.

Known incomplete areas include deferred import evaluation, full formatter APIs,
search collation tailoring, per-locale support for collation extensions, and
plural locale coverage beyond ICU's compiled data. These remain test failures;
they are not skipped or replaced with successful stubs.

## Completed full measurement

The complete optimized run passes **98,010 / 102,956 (95.1960%)**: 4,944
failures, two timeouts, and zero skips, crashes, or harness errors. It exceeds
the 97,809-pass minimum by 201 variants. The user’s minimum 95% goal is achieved.
All variants in the pinned corpus remain in the denominator.

Comparison against the preceding 96,103-pass full snapshot proves **1,907 gains
and zero losses**, with identical variant identities. The source tree and
frozen worker still match the saved source manifest. The runner, corpus,
feature selections, fuel/loop budgets, allocation caps, and timeout were not
changed.

Worker SHA-256:
`dda45144ab2a946433f3819ad4041fd103451de1d17e051f910907761b29ec9d`.
Measured source is commit `8de62fd` plus the archived `measured.patch`, SHA-256
`d104c682349b5004947afdf4fe57cffaac7f010d1a98c10965425635558f11ac`.
The patch was frozen before documentation and evidence-only changes.

[Compact current result](../tools/test262/latest.json) and
[archived evidence](../tools/test262/evidence/unicode-annex-builtins/)
contain the complete compressed report, previous full report, every status
transition, source patch/manifest, build environment, and validation logs.
The final integration run passes all 68 tests; the Node-API host feature check
and bytecode verifier/parity tests pass. The existing native-addon fixture
failure described above remains.

Reproduction:

```sh
cargo build --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --output artifacts/test262/unicode-annex-builtins-full.json
```

This is the full-corpus native development result. It does not assert complete
ECMAScript support or unmeasured cross-tier/wasm conformance.
