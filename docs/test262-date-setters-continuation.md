# Date setters continuation

Historical implementation checkpoint. Its full-corpus counts below describe
that earlier source snapshot; see
[latest.json](../tools/test262/latest.json) for the current result.

Continues the
[declaration-completion continuation](test262-declaration-completion-continuation.md)
from `1bd0a34`. Triage of the fresh full-corpus run at that commit (74,993 /
102,956, 72.84%) showed the `built-ins/Date` cluster failing largely on
unimplemented methods, so this turn implements them.

Only `setTime` existed. All fourteen `setFullYear`/`setMonth`/`setDate`/
`setHours`/`setMinutes`/`setSeconds`/`setMilliseconds` setters (plus UTC
twins, identical here since the sandbox has no local timezone) are now
implemented in `crates/napi-vm-core/src/builtins/date.rs` through shared
`MakeDay`/`MakeTime` helpers with `TimeClip` finish steps. Argument handling
follows the spec order: the instant is read before any coercion, provided
arguments coerce left to right (a missing leading argument is `undefined`,
hence `NaN`), the rest default from the current components, a `NaN` instant
stays `NaN` (except `setFullYear`, which falls back to the epoch), and
year/month/date results preserve the time of day. Unlike the constructor,
setters apply no two-digit year mapping. Each setter carries its spec
`length`. `tests/date_setters.rs` pins Node-verified UTC values,
local/UTC equivalence, coercion order, `NaN` handling, in-place updates,
receiver errors, and lengths in both tiers.

Also in this turn: getters now enforce `thisTimeValue` (a `TypeError` for
non-`Date` receivers, including the prototype object);
`Date.prototype[Symbol.toPrimitive]` is implemented with the specified
string-first/default and number-first hint orders, backed by a new
`(Value::Date, Value::Symbol)` prototype-chain arm in the property lookup
(instance symbol reads previously missed); Annex B `getYear`/`setYear`
(truncated 0–99 maps into the 1900s, `NaN` blanks the date); and
`toGMTString` is installed as the same function object as `toUTCString`.

Focused pinned-corpus measurement (revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers, five-second
timeouts, no feature skips) over `built-ins/Date` and `annexB/built-ins/Date`
passes 1,130 / 1,236 variants (91.42%) in
`artifacts/test262/date-setters-focused.json`. Against the matching scope of
the full run at `1bd0a34`, failures drop from 638 to 106 with zero
regressions. The remainder is scattered follow-up material (other missing
formatting methods, constructor edge cases, descriptor checks), not one
mechanism.

Full conformance is not achieved, and no new full-corpus measurement was run
for this continuation; the fresh full run at `1bd0a34` recorded 74,993 /
102,956 passes (72.84%) with zero skips, crashes, or harness errors.

Rust verification: the new `date_setters` suite passes (10 tests), along
with the neighboring integration suites; `cargo fmt --check` is clean and
Clippy reports only pre-existing warnings. The
`object_spread::rest_enumerates_after_named_bindings_and_excludes_symbol_keys`
failure was reproduced on the untouched tree and is pre-existing.
