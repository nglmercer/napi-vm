# Test262 history

Earlier Test262 notes were one document per experiment. The current status is in
[`test262-full-pass-progress.md`](test262-full-pass-progress.md). The removed notes
were superseded: several describe the removed Python runner, and their
measurements no longer match the pinned revision. They were removed in the
commit that introduced this file. To read one, use the commit that last
contained it:

```sh
git show 9bc2103:docs/<name>
```

| Document | Title |
| --- | --- |
| `test262-99-90-progress.md` | Test262 99.90% continuation |
| `test262-abstract-equality-continuation.md` | Abstract equality continuation |
| `test262-array-elisions-continuation.md` | Array elisions and observable searches: incremental conformance work |
| `test262-array-fill-timeouts.md` | Array fill timeout continuation |
| `test262-array-logical-lengths-continuation.md` | Logical lengths in Array methods |
| `test262-array-mutation-search.md` | Array mutation, search and map/filter quick wins |
| `test262-array-string-invocation-continuation.md` | Array string invocation and intrinsic fallback |
| `test262-async-cycle-continuation.md` | Async cycle outcomes and imported namespaces |
| `test262-async-delegation-continuation.md` | Async generator delegation continuation |
| `test262-async-iterator-disposal-continuation.md` | Async iterator disposal |
| `test262-binary-data.md` | Binary-data continuation |
| `test262-bound-function-chains-continuation.md` | Bound function target chains |
| `test262-byte-modules-continuation.md` | Immutable byte modules |
| `test262-checkpoint-status.md` | Conformance development checkpoint |
| `test262-class-property-layout-continuation.md` | Class property layout continuation (2026-10-18) |
| `test262-conformance-followup.md` | Conformance follow-up |
| `test262-conformance-validation.md` | Test262 conformance validation: merged PR #22 baseline |
| `test262-control-flow-completion-continuation.md` | Control-flow completion continuation |
| `test262-core-constructor-metadata-continuation.md` | Core constructor metadata and Array unscopables |
| `test262-date-setters-continuation.md` | Date setters continuation |
| `test262-declaration-completion-continuation.md` | Declaration completion and binding-resolution continuation |
| `test262-default-export-names-continuation.md` | Default export declaration names continuation |
| `test262-deferred-import-continuation.md` | Deferred module evaluation continuation |
| `test262-dense-array-regexp-continuation.md` | Dense array and regular expression continuation toward 98% |
| `test262-descriptor-map-continuation.md` | Descriptor map continuation |
| `test262-duration-continuation.md` | DurationFormat continuation toward the full-suite 98% goal |
| `test262-dynamic-defer-continuation.md` | Dynamic deferred imports and asynchronous dependencies |
| `test262-dynamic-import-attributes-continuation.md` | Static and dynamic import attributes continuation |
| `test262-empty-accessors-continuation.md` | Empty accessor descriptors and duplicate parameters |
| `test262-export-patterns-continuation.md` | Exported destructuring bindings |
| `test262-export-resolution-continuation.md` | Cyclic and ambiguous export resolution |
| `test262-finally-completions-continuation.md` | Nested finally completion continuation |
| `test262-for-in-enumeration-continuation.md` | Descriptor-aware for-in enumeration |
| `test262-foundations-completion.md` | Test262 foundation completion: work in progress |
| `test262-foundations-followup.md` | Foundations follow-up after merged PR #24 |
| `test262-function-proxy-metadata-continuation.md` | Function and Proxy constructor metadata |
| `test262-generic-array.md` | Generic Array quick wins |
| `test262-harness-error-continuation.md` | Harness error classification |
| `test262-import-meta-identity-continuation.md` | Per-module import.meta identity |
| `test262-instanceof-primitive-order-continuation.md` | Instanceof primitive operand ordering |
| `test262-intl-regexp-protocol-continuation.md` | Intl locale and RegExp protocol continuation |
| `test262-json-raw.md` | Raw JSON continuation |
| `test262-json-reviver.md` | JSON reviver continuation toward 90% Test262 |
| `test262-lexical-scopes-continuation.md` | Lexical scope continuation |
| `test262-live-collections-continuation.md` | Live collection iteration continuation |
| `test262-mapped-arguments-continuation.md` | Mapped arguments continuation |
| `test262-math-array-number.md` | Math, Array and Number quick wins |
| `test262-math-quick-wins.md` | Test262 Math quick wins |
| `test262-math-sum-precise-continuation.md` | Math.sumPrecise |
| `test262-named-evaluation-continuation.md` | Named evaluation and exponentiation continuation |
| `test262-namespace-tdz-continuation.md` | Namespace descriptor TDZ continuation |
| `test262-node-addon-descriptors-continuation.md` | Native object descriptor continuation |
| `test262-node-addon-snapshots-continuation.md` | Native-addon snapshot continuation |
| `test262-numeric-property-keys-continuation.md` | Numeric property key conversion continuation |
| `test262-object-constructor-metadata-continuation.md` | Object constructor metadata |
| `test262-object-copy-continuation.md` | Object copy semantics and fresh full-corpus evidence |
| `test262-object-enumeration-continuation.md` | Object enumeration continuation |
| `test262-object-integrity-continuation.md` | Object integrity and iterator continuation |
| `test262-object-locale-string-continuation.md` | Object locale-string accessor invocation |
| `test262-object-metadata-continuation.md` | Object and Math metadata continuation |
| `test262-optional-chain-references-continuation.md` | Optional-chain references continuation |
| `test262-phase1-host-realms-gc.md` | Phase 1 host, realms, and GC audit |
| `test262-phase2-contexts.md` | Phase 2 contextual grammar and static semantics |
| `test262-phases-1-3.md` | Test262 phases 1–3: foundation implementation |
| `test262-promise-host-work-continuation.md` | Promise capability host work continuation |
| `test262-public-hash-keys-continuation.md` | Public hash-named property continuation |
| `test262-regexp-group-name-escapes-continuation.md` | Capture names use Unicode identifier escapes |
| `test262-regexp-modifiers-continuation.md` | Scoped regular-expression modifiers continuation |
| `test262-relational-operators-continuation.md` | Relational operators continuation |
| `test262-relative-time-continuation.md` | RelativeTimeFormat continuation toward the full-suite 98% goal |
| `test262-remaining-quick-wins.md` | Remaining Test262 quick wins |
| `test262-segmentation-list-buffer-continuation.md` | Segmenter, list formatting, captures, and buffer transfer continuation |
| `test262-shadow-realm-continuation.md` | ShadowRealm Test262 continuation |
| `test262-sparse-array-indices-continuation.md` | Sparse array indices continuation |
| `test262-static-source-imports-continuation.md` | Source-phase module imports |
| `test262-string-semantics-continuation.md` | String coercion, metadata, and legacy escapes |
| `test262-super-reference-order-continuation.md` | Super reference receiver order continuation |
| `test262-tagged-template-construction-continuation.md` | Tagged template constructor precedence |
| `test262-tail-calls-continuation.md` | Tail-call execution continuation |
| `test262-template-object-cache-continuation.md` | Realm-owned tagged template cache |
| `test262-template-object-integrity-continuation.md` | Tagged template object integrity |
| `test262-temporal-datetime.md` | Temporal PlainDateTime continuation |
| `test262-temporal-instant.md` | Temporal Instant continuation |
| `test262-temporal-plain-date.md` | Temporal PlainDate continuation |
| `test262-temporal-time-duration.md` | Temporal PlainTime and Duration continuation |
| `test262-temporal-yearmonth-monthday.md` | Temporal PlainYearMonth and PlainMonthDay continuation |
| `test262-temporal-zoned.md` | Temporal ZonedDateTime continuation |
| `test262-typed-array-buffer-coercion-continuation.md` | TypedArray buffer argument coercion |
| `test262-typed-array-constructor-calls-continuation.md` | TypedArray constructors require new |
| `test262-typed-array-method-installation-continuation.md` | TypedArray method installation |
| `test262-typed-array-numeric-keys-continuation.md` | TypedArray ordinary numeric-looking keys |
| `test262-typed-array-shared-coercion-continuation.md` | TypedArray shared buffer coercion |
| `test262-typed-array-slice-copy-continuation.md` | TypedArray slice copy order |
| `test262-unicode-annex-builtins-continuation.md` | Unicode, Annex B, and built-in conformance continuation |
| `test262-var-destructuring-continuation.md` | Var destructuring reference order continuation |
| `test262-verified-builtins-continuation.md` | Detached-checkout built-in verification |
| `test262-weak-map-timeouts.md` | Historical timeout investigation |
| `test262-with-bindings-continuation.md` | With and Proxy binding continuation |
