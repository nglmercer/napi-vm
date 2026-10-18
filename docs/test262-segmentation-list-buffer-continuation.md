# Segmenter, list formatting, captures, and buffer transfer continuation

Historical continuation snapshot. Its recorded full run and release checkpoint
describe the earlier implementation state; the current result is in
[latest.json](../tools/test262/latest.json) and the newer
[ShadowRealm/full-suite continuation](test262-shadow-realm-continuation.md).

At this checkpoint, the target was at least 95% of all variants in the pinned
Test262 corpus. The threshold was 97,809 / 102,956 and had not yet been
achieved.

## Completed full measurement

`artifacts/test262/intl-regexp-full.json` measures 95,619 / 102,956 passes
(92.8737%), 7,335 failures, and two timeouts. There are no skips, crashes, or
harness errors. Worker SHA-256 is
`2d63bc8630ae198c0e90ceacf1d7549f32b4b94d32404ee9293935eae4b17f78`.
The exact source snapshot is preserved by the associated source manifest/patch.

Comparison with the previous full run proves 1,302 gains and four losses,
net +1,298, with all variant identities matching. The four losses are
RegExp.prototype.compile resetting the old internal index instead of the new
observable lastIndex property. This continuation corrects that regression.

## Implementation

- Intl.Segmenter uses ICU4X 2.3 word, sentence, and grapheme boundaries on UTF-16
  input. Branded segments support containing(), independent iterators, fresh
  records, UTF-16 indices, word-like classification, and collection between
  creation and consumption. Locale/options conversion and constructor prototype
  lookup retain JavaScript-observable ordering.
- Intl.ListFormat supports conjunction, disjunction, unit, and all three styles,
  with ICU4X locale patterns and contextual Spanish/Hebrew alternatives. Format
  and formatToParts use observable guest iteration and close the iterator when a
  non-string element is produced. ICU element parts are replaced with their
  original UTF-16 strings, preserving lone surrogates and empty elements.
- CLDR available locales and default-content locales support shared locale
  negotiation and supportedLocalesOf. Source URLs/hashes and generation steps
  are in the existing locale data generator; the Unicode license is retained.
- Regexp named captures can share a name in disjoint alternatives approved by
  the syntax validator. Backreferences select the participating capture and
  resolve forward references. Capture records follow source order. Repetition
  clears captures inside its quantified body before the next iteration.
- Capture-name identifier escapes use their decoded StringValue for both
  validation and matching, including surrogate pairs. Annex B identity escapes
  remain literal when a non-Unicode pattern has no named captures.
- ArrayBuffer.transfer preserves source resizability and its maximum; transferToFixedLength
  creates a fixed result. Both convert length, recheck detachment, copy/truncate
  or zero-extend bytes, use the method realm's intrinsic prototype, and detach
  all source views. Species and mutable constructor properties are not read.
  ArrayBuffer constructor/method/tag metadata is corrected.

No corpus, harness, execution/allocation cap, worker fuel, loop budget, process
timeout, feature skip, or denominator was changed.

## Focused verification

`artifacts/test262/intl-segmentation-buffers-debug.json` selects ArrayBuffer,
SharedArrayBuffer, RegExp prototype/named groups, String.prototype, Annex B
RegExp, Locale/getCanonicalLocales, Segmenter, and ListFormat. It passes
4,448 / 4,704 variants, with 464 gains and zero losses against the completed
full run. All 158 Segmenter and 160 ListFormat variants pass. Focused percentages
are not overall compatibility measurements.

All 20 integration tests pass across arraybuffer_transfer, intl_locale,
intl_segmenter_list, and regexp_protocol. Tests cover length coercion with
resize/detach side effects, species avoidance, collection of segment iterator
state, contextual formatting, original UTF-16 parts, invalid iterator elements,
escaped/duplicate/forward named references, and capture reset across repetition.
Formatting and whitespace checks pass. The previously recorded native-addon
core fixture failure remains outside this continuation's changes.

Remaining broad gaps include other Intl formatters, immutable buffers, binary
encoding APIs, Annex B hoisting, module phases, Unicode regexp sets, and bounded
execution failures. A full optimized run was still required at this checkpoint.
The current complete result and remaining failures are linked above.

## Release checkpoint

`artifacts/test262/segmentation-list-buffers-release-focused.json` reproduces
4,448 / 4,704 passes with the optimized worker, SHA-256
`cbba27816cef57e30ae11d2431e9a2d270f975351ef03222d478772805928dd1`.
All 53 selected integration tests pass, adding engine_boundary,
test262_worker, promise_semantics, and resource_semantics to the 20 above.

The fresh complete run writes
`artifacts/test262/segmentation-list-buffers-full.json` and its matching
`.checkpoint.jsonl` journal. The durable worker is
`artifacts/test262/segmentation-list-buffers-worker`; source file hashes,
base revision, patch SHA-256, and worker hash are preserved in the matching
`-source.json` manifest and `-source.patch`. Poll the existing process before
attempting any resume, and use this durable worker if resume is necessary.
