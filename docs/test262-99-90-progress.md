# Test262 99.90% continuation

Target for the pinned 102,956-variant corpus: at least **102,854 passes**.
Keep all selected variants in the denominator, with no feature skips.

## Measurement history

A full run of source `e2b1cd6ad98378df9c6b584a3e00b6a8b01e7d42`
observed 102,378 passes, 532 failures, 42 timeouts, four crashes, zero skips,
and zero harness errors (99.438595%). It used Rust 1.98.1, two workers,
10-second timeouts, and worker SHA-256
`fa2d230cf8123c7d867674c381711f26b07f5ea275a584def7e8e87a8b1543a1`.
The execution environment was reset before its report was uploaded. The raw
evidence is unavailable, so this observation does not replace `latest.json`.

The first fix checks the output limit on numeric and named capture substitutions
before their loop continues. Without this check, repeated `$1` tokens can expand
one match to tens of GiB and terminate the worker instead of raising RangeError.
Before the reset, its native regression passed and the String replacement plus
`staging/sm/String/replace-math.js` selection passed 110/110 variants. Revalidation
on the current branch remains required; no new full score is claimed.

## Restored verification and registry memory

On the restored branch, the capture-limit native regression passes again.
The two BMP/eval files pass 4/4 in a one-worker diagnostic, but the worker reaches
5,328,296 KiB peak RSS. This is enough to exhaust the 8 GiB execution environment
when two workers run together. The raw focused report is archived under
`tools/test262/evidence/conformance-99-90/`.

A native regression demonstrates the allocation registry growing from 1,289 to
121,301 entries during 20,000 transient-array iterations without cycle collection.
The registry now periodically drops dead Weak registrations, whose Rc allocations
otherwise remain resident. It never clears live cells or runs the cycle collector
during execution. The pruning interval scales with the retained live registry.
The regression, heap root/barrier tests, iterative-drop tests, capture-limit test,
and formatting checks pass. A release build of clean commit `e50d4c3` passes the
same four BMP variants with the same one-worker/10-second configuration. Peak
RSS falls from 5,328,296 KiB to 37,688 KiB (99.29% lower). The report and memory
measurement are archived alongside the before report. Broader two-worker
conformance verification passes 842/852 selected RegExp/String variants with
two workers and 10-second timeouts, with ten semantic failures and zero crashes,
timeouts, skips, or harness errors. Its raw compressed report is archived.

## Temporal locale formatting

The unchanged release baseline for `intl402/Temporal`, `intl402/DateTimeFormat`,
and `built-ins/Temporal` passes 13,548/13,758 variants, with 210 failures and no
other nonpassing outcomes. The compressed baseline is archived with its worker
digest and configuration.

Temporal locale methods now use the existing Intl date/time and duration
formatters instead of ISO serialization, including receiver brands, calendar
compatibility, style conflicts, and ZonedDateTime's own time zone. Numeric hour
formatting retains the locale pattern's leading zero rather than stripping it
from 24-hour clock output. Four new native regressions pass through both AST
and prepared evaluation. The clean `f6b780c` release passes 13,706/13,758
variants: 158 new passes, zero lost passes, and 52 semantic failures with no
crashes, timeouts, harness errors, or skips. The after report and per-variant
comparison are archived. This focused result does not establish the full-suite
percentage.

## Remaining gates

- [ ] Revalidate restored changes and archive focused evidence.
- [ ] Reproduce SIGKILL outcomes in the BMP RegExp/eval loops and identify cause.
- [ ] Triage the remaining Temporal, staging, Annex B, and DateTimeFormat cases.
- [ ] Validate semantic changes in AST and prepared bytecode where applicable.
- [ ] Run the complete pinned corpus on a clean committed candidate and archive it.
- [ ] Reach at least 102,854 passes with zero crashes, harness errors, and skips.

Push each verified implementation block and its evidence to this draft branch.
Keep runtime capabilities optional, execution limits enforced, and AST fallback.

## HTMLDDA host support

The optional Test262 host now exposes a callable object with the HTMLDDA brand.
Only boolean coercion, typeof, and loose null/undefined comparisons change;
strict equality, object operations, nullish checks, inheritance and proxies
retain ordinary object semantics. Three native regressions pass in AST and
prepared modes. The matched baseline selects 84 variants and passes 30, with
54 failures. The clean `2a56bc8` release passes all 84 variants: 54 new
passes and zero lost passes, failures, timeouts, crashes, harness errors or
skips. Both reports and their comparison are archived.

## Temporal reference dates and default hour cycles

Formatting now converts PlainDate, PlainYearMonth and PlainMonthDay reference
ISO dates directly from the library's exact UTC nanoseconds. This preserves
non-Gregorian calendar conversion and dates outside Date's clipped range,
without constructing an invalid midnight PlainDateTime. The formatter retains
its effective hour cycle internally even when resolvedOptions omits time
fields, so default h24 midnight is rendered as 24. Six native regressions pass
in AST and prepared modes. The clean `4a30aa4` release passes 13,736/13,758
variants: 30 new passes, zero lost passes, and 22 semantic failures with no
timeouts, crashes, harness errors, or skips. The after report and comparison
are archived. Core library Clippy passes with warnings denied.

## Duration relative dates and range formatting

Duration relativeTo now shares calendar-slot conversion and date-field
validation with other Temporal entry points, including era/eraYear ordering.
Range collapsing compares displayed fields, including day periods and
fractional seconds, instead of raw ISO ranks. Fixed zero-offset zones use
ICU's GMT identity and nonzero offsets use localized offset names. Nine native
regressions pass through explicit Script compilation, normal prepared
evaluation and AST execution. An earlier native regression reproduces the
range failure only in release, so release verification is required for this
block before claiming its conformance gains.
