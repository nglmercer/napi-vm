# Intl locale and RegExp protocol continuation

Historical continuation snapshot. The full measurement and goal state below
describe this earlier checkpoint; see [latest.json](../tools/test262/latest.json)
and the newer [ShadowRealm/full-suite continuation](test262-shadow-realm-continuation.md)
for the current complete result.

At this checkpoint, the user goal was at least 95% passing on the complete
pinned Test262 corpus, and it remained unachieved.

## Completed full measurement

`artifacts/test262/async-resource-regexp-full.json` measures 94,321 / 102,956
passes (91.6129%) with 8,633 failures and two timeouts. There are zero skips,
crashes, or harness errors. All variant identities match the preceding full
baseline; comparison proves 1,521 gains and zero losses. Worker SHA-256:
`c987b1a2f786addf784f377862bb7f92f8599e5c8e813d521e422e4bdcd275b8`.

This completed snapshot includes Promise capability/species/combinator work,
Array.fromAsync, async disposal and resource statements, and scoped regexp
modifiers. It predates the Intl and generic regexp protocol changes below.
The full minimum for 95% is 97,809 passes: 3,488 more than this snapshot.

## New implementation

- `Intl.Locale` uses branded internal slots, observable JavaScript tag and option
  conversion, CLDR canonicalization, Unicode/transform keyword aliases, reserved
  long language subtags, getters, and maximize/minimize. Locale-info methods
  implement calendar preferences, hour cycles, week information, and text
  direction, including region override/subdivision precedence and fresh results.
- ICU4X 2.3.1 handles locale syntax and alias data. Generated CLDR release-48 data
  supplies keyword aliases, full likely-subtag mappings, calendar preferences,
  clock preferences, and week data. The generator records source hashes and the
  Unicode license is retained under `tools/intl/`.
- Generic `RegExp.prototype` match, search, matchAll, replace, split, flags,
  toString, and test observe property access, conversions, exec overrides,
  species constructors, and errors. String methods dispatch through the symbol
  protocols in the specified order.
- `matchAll` returns a branded, lazy iterator with traced matcher state rather
  than eagerly collecting an array. Iterator methods and the iterator prototype
  carry the appropriate descriptors and prototype chain.
- RegExp `lastIndex` assignments retain the uncoerced value. Builtin exec performs
  ToLength on use, observes writability, and leaves empty-match advancement to
  the consuming protocol. Match arrays support indices and null-prototype named
  capture records; source serialization escapes slash and line terminators.
- Property reads resolve paired literal accessors when the setter precedes the
  getter. Regexp nesting accounting no longer charges sequence/repeat helper
  frames as additional syntax nodes; matching uses growable native stacks and
  retains the node-depth and backtracking budgets.

No corpus, harness, feature skips, execution/allocation caps, worker fuel, loop
budget, or process timeout were changed.

## Validation and open work

The broad debug selection in `artifacts/test262/intl-regexp-string-debug-v2.json`
passes 3,342 / 3,538 variants, with 1,000 gains and zero losses versus the completed
full snapshot. This is a selected-subset result, not an overall compatibility
measurement. This measurement includes the string coercion fixes.

Six regexp protocol tests pass, including collection between iterator creation
and consumption, generic exec dispatch, paired getter/setter access, replacement,
indices, and lastIndex conversion. Four Intl locale tests pass, including alias
handling, conversion order, branding, reserved languages, and region preferences.
The core library run passes 355 tests and fails the previously documented native
addon fixture `loads_and_invokes_a_real_node_api_addon` with
`Cannot redefine property: prototype`.

Remaining failures at this checkpoint included other Intl constructors and formatting, Locale
collation/number-system/time-zone info methods, regexp Unicode sets and duplicate
named groups, parser/exotic-object gaps, and tests exceeding current budgets.
The current complete release run verifies a later combined implementation.
Use the linked current result for its percentage and remaining gap; the
97,809-pass threshold above was the goal for this historical checkpoint.

## Release checkpoint

The optimized focused run `artifacts/test262/intl-regexp-string-release-final.json`
reproduces 3,342 / 3,538 passes. Worker SHA-256:
`2d63bc8630ae198c0e90ceacf1d7549f32b4b94d32404ee9293935eae4b17f78`.
All 43 integration tests pass across engine_boundary, intl_locale,
promise_semantics, regexp_protocol, resource_semantics, and test262_worker.
The runner's 12 Python tests pass. Formatting and diff whitespace checks pass.

The new complete run writes `artifacts/test262/intl-regexp-full.json` and
`artifacts/test262/intl-regexp-full.checkpoint.jsonl`. Its durable worker is
`artifacts/test262/intl-regexp-worker`; subsequent builds can safely use target/
without preventing resume of this snapshot. Poll the existing process before
attempting resume. Source file hashes, base revision, patch hash, and worker hash
are in `artifacts/test262/intl-regexp-source.json`; its complete source patch is
`artifacts/test262/intl-regexp-source.patch`.
