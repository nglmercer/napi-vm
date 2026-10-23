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
and formatting checks pass. Release memory and focused conformance verification
of this registry change remain pending.

## Remaining gates

- [ ] Revalidate restored changes and archive focused evidence.
- [ ] Reproduce SIGKILL outcomes in the BMP RegExp/eval loops and identify cause.
- [ ] Triage the remaining Temporal, staging, Annex B, and DateTimeFormat cases.
- [ ] Validate semantic changes in AST and prepared bytecode where applicable.
- [ ] Run the complete pinned corpus on a clean committed candidate and archive it.
- [ ] Reach at least 102,854 passes with zero crashes, harness errors, and skips.

Push each verified implementation block and its evidence to this draft branch.
Keep runtime capabilities optional, execution limits enforced, and AST fallback.
