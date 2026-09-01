# What's next

State as of `630131f`, with CI results from run 33517882112. The roadmap
(`docs/roadmap.md`) is the feature tracker; this is the shorter list of what
to *do*, in the order it is worth doing.

## Blocking — CI is red

Three failures on
[run 33514005272](https://github.com/hernan-lc/napi-vm/actions/runs/33514005272),
two of them still red on
[run 33517882112](https://github.com/hernan-lc/napi-vm/actions/runs/33517882112).
None is caused by the feature work; all are gaps that were invisible until
now. The last CI run before them was **24 Aug**, and every commit from
`f738645` (31 Aug) onward — the quality-gate job, the coroutine generator
implementation, lexical scoping — landed without CI ever seeing them.

### 1. `tsc --noEmit` on the playground — fixed, confirmed green

`playground/public/src/vm.ts` imports `/pkg/napi_vm.js`, which `wasm-pack`
generates. No CI job built that package, so the check could never pass on a
clean checkout. It looked green locally only because a developer tree has a
stale `playground/pkg` from an earlier build.

**Fixed** in `630131f`: a `playground` job builds the package, type-checks
against it, and runs the browser suite — which also gets `tests/wasm/` running
in CI for the first time. Both `Lint and type-check` and `Browser build and
tests` are green on run 33517882112.

### 2. Windows x86_64 `generator_stress` — `STATUS_ACCESS_VIOLATION`, test now named

`630131f` ran the suite single-threaded so the harness would flush, and the
log names it:

```
test an_exhausted_for_of_runs_finally_once ... ok
error: test failed, to rerun pass `--test generator_stress`
  process didn't exit successfully: generator_stress-….exe --test-threads=1 --nocapture
  (exit code: 0xc0000005, STATUS_ACCESS_VIOLATION)
test cloned_generator_values_share_one_coroutine ...
```

**`cloned_generator_values_share_one_coroutine`** (`tests/generator_stress.rs:195`).
It loops 100× building `counter()`, calling `next()` three times, and dropping
it — so every iteration drops a coroutine *suspended at `yield 3`*, which sends
`Coroutine::drop` down `force_unwind_slow`: resume the body with a
`ForcedUnwind` panic and unwind its stack.

libtest runs alphabetically, and the four tests that pass before it are the
four that never abandon a suspended generator in bulk. So this is simply the
first test to force-unwind repeatedly — the other abandonment tests
(`generators_abandoned_while_suspended`, `nested_generators_abandoned_mid_flight`)
sort later and have still never run on Windows.

This is **pre-existing**, from `563033a` ("The generator race is closed",
31 Aug), which replaced the OS-thread generators with `corosensei` stackful
coroutines. That commit has never run on Windows CI; the last green Windows
run (24 Aug) predates it.

What was ruled out locally:

| Hypothesis | Result |
|---|---|
| Coroutine stack overflow | No — the suite passes with the stack cut to 256 KB |
| Flakiness / a race | No — 5×17 runs clean on Linux |
| A newer `corosensei` fixes it | No — 0.3.4 is the latest |
| A known upstream Windows bug | None filed |
| "Any force-unwind faults" | No — `a_generator_exhausting_the_loop_budget_unwinds_cleanly` does exactly one and passes |

That last row is the useful one: a single force-unwind survives, a hundred do
not, which points at something *cumulative* rather than at the unwind itself.

**Leading hypothesis (unverified).** On Win64 an exception cannot cross the
stack-switch trampoline — `corosensei/src/arch/x86_64_windows.rs:185` says so
outright: *"we can't actually use this to throw an exception across stacks
because the unwinder will not update the TEB fields when switching stacks."*
So the forced-unwind panic is caught at the coroutine root and the four TEB
stack fields (`gs:[0x08]` StackBase, `gs:[0x10]` StackLimit,
`gs:[0x1478]` DeallocationStack, `gs:[0x1748]` GuaranteedStackBytes) are
saved and restored by hand around every switch. If the unwind path leaves the
thread's TEB describing a coroutine stack that `DefaultStack::drop` then
`VirtualFree`s, the fault lands later, on unrelated code. That fits every
observation: harmless once, fatal after N, and structurally impossible to
reproduce on Linux, which has no TEB.

**Shipped: `tests/coroutine_backend.rs`, the decisive experiment.** It drives
`corosensei` directly — no interpreter — through six stages ordered by how
much of the backend they need, and runs single-threaded on Windows x86_64
*before* the stress suite:

| | stage | what it adds |
|---|---|---|
| `t1` | run to completion | switching only, no unwind |
| `t2` | dropped before starting | drops the initial closure, not a live stack |
| `t3` | dropped while suspended | the forced unwind, 200× — the shape that dies |
| `t4` | destructors on the unwound stack | proves it unwinds rather than discarding |
| `t5` | suspended 64 frames deep | a long unwind, as a real `yield` produces |
| `t6` | nested, both suspended | unwinding a stack that owns another stack |

**Verdict, from
[run 33529798397](https://github.com/hernan-lc/napi-vm/actions/runs/33529798397):
all six pass on Windows x86_64**, and `generator_stress` still faults in the
same place. 200 forced unwinds with destructors, 64-frame-deep suspensions and
nested coroutines survive with no interpreter attached.

So `corosensei`'s Windows forced-unwind is sound and **the fault is ours** —
in what runs on the coroutine stack *during* the unwind: the `Interpreter`,
`Env` and `Value` destructors of the generator's own body. The TEB-corruption
hypothesis recorded here earlier is refuted; that is what the experiment was
for. Nothing to file upstream.

Where to look, in order:
`GeneratorInner`'s teardown, `Value::drop` (which drains children onto an
explicit work stack rather than recursing), and the `Env` chain a body's
frame holds — every one of those releases `Rc`s the *driver* on the main
stack still owns.

**Next, and pushed with this:** two things that answer "where" without another
guess.

1. `generator_stress` gains `abandoning_a_few_suspended_generators` (10) and
   `abandoning_many_suspended_generators` (100) — the same abandonment with
   the aliasing removed. They sort before `cloned_…`, so the last name the
   log prints separates *structural* (10 is enough) from *cumulative* (only
   100 faults) from *specific to that test's aliasing* (both pass).
2. An `if: failure()` step re-runs the faulting binary under `cdb`, the
   Windows SDK debugger on the runner image, and prints `!analyze -v` plus
   `kP 100`. `CARGO_PROFILE_RELEASE_DEBUG: line-tables-only` on that job is
   what makes those frames readable. It is diagnostic only — it cannot change
   the verdict, and it is a no-op once the suite is green.

An access violation is not a Rust panic: no backtrace, no message, just an
exit code. That step is the difference between reading the faulting frame and
guessing at it again.

**Options if it proves hard:** mark `generator_stress` as
`#[cfg_attr(windows, ignore)]` with the reason recorded, and file it — better
than leaving CI red or, worse, deleting the coverage that found it.

### 3. Windows aarch64 — `corosensei` does not build at all — fixed

Newly visible, same dependency, but not a debugging problem — a support gap:

```
error: Unsupported target        (corosensei/src/arch/mod.rs:177, compile_error!)
error[E0308]: mismatched types   ×2
error: could not compile `corosensei` (lib) due to 3 previous errors
```

`corosensei` 0.3.4 ships `arch/x86_64_windows.rs` and `arch/x86_windows.rs`,
but its aarch64 backend is gated `all(target_arch = "aarch64", not(windows))`.
There is no ARM64-Windows stack-switching backend, so
`aarch64-pc-windows-msvc` could not compile — the job died in `Build native
module` before any test ran. This also failed on run 33514005272; the earlier
version of this document counted only two failures and missed it.

**Fixed** by making the fallback a property of the *target's capabilities*
rather than of `wasm32` specifically. The source used to ask
`cfg(not(target_arch = "wasm32"))` at ~50 sites, which conflated "is the
browser" with "has no stack switching". Those are now
`cfg(stackful_coroutines)`, emitted by a new `build.rs` from `corosensei`'s
own support table; `Cargo.toml` scopes the dependency to exactly the same set,
and `build.rs` says so in a comment, because claiming support the manifest
does not ship fails to compile. `aarch64-pc-windows-msvc` now takes the same
buffered generator path `wasm32` has always taken.

Verified locally against `aarch64-pc-windows-msvc` (with and without the
`napi` feature), `wasm32-unknown-unknown` and `wasm32-wasip2`.

The cost is real and worth stating: generator semantics on ARM64 Windows now
differ from x86_64 Windows in the ways `call::generator_next` documents —
side effects happen at the first `next()`, `next(v)` cannot send a value in,
and an unbounded generator trips the loop budget. That is the same trade-off
the browser build already makes. The alternatives were dropping the target
outright, or writing an `aarch64_windows.rs` backend upstream — the correct
fix, and considerably more work than this.

## Then — the one remaining roadmap capability

**True generator suspension on `wasm32`.** Today the browser build runs a
generator body to completion on the first `next()` and buffers its yields.
Values, `for…of`, spread and `yield*` all work; the difference is observable
and documented. Closing it needs one of:

- an explicit continuation stack in the evaluator (general, but a rewrite of
  the core),
- a CPS transform of generator bodies (narrower, but the subtle cases are
  where such transforms go wrong), or
- binaryen's Asyncify (cheapest, but a build dependency this repo does not
  have, and it costs size and speed on every path).

A *partial* transform is worse than today: it would make a generator's
semantics depend on whether its body matched a pattern.

## Smaller, in order

1. **Re-entrant host calls.** A VM function exported to the host is refused
   with "VM is busy" if called from inside a VM execution. Generators at the
   N-API boundary need the same mechanism.
2. **A pretty-printer that can rewrap lines.** Today's formatter only
   re-indents, because the parser does not retain comments and a formatter
   that deletes them is worse than none.
3. **`Intl`, `Object.groupBy`** and the other recent library additions. None
   are load-bearing for the sandbox.

## Decisions waiting on you

**`Function` and `Proxy` are now implemented.** A test named *"cannot escape
sandbox via constructor"* had asserted `Function` was inert; I implemented it
and rewrote that test. Containment was verified first — `new Function('return
this')()` is `undefined`, `process`/`require` reach only the pre-existing inert
stubs, and the `f.constructor.constructor` chain the classic escape travels
does not exist at all. If the inert `Function` was deliberate policy rather
than an unimplemented stub, revert `7ab6b0b`.

## Health

- **Tests:** 1378 (bun) · 184 Rust · 27 LSP protocol · 17 Node-compat ·
  19 generator stress · 7 WASM · 6 coroutine backend.
- **Gate:** `lint`, `build`, `test`, `test:node`, `test:rust`, `test:wasm` —
  all green locally, on every target including `wasm32`, `wasm32-wasip2` and
  `aarch64-pc-windows-msvc`.
- **Performance:** within ~4% of the pre-feature baseline after `2e83a1d`,
  which removed a heap allocation from every property write. Re-measure with
  `npm run bench` after any change to `assign_member` or `Value`.
- **Release:** still `0.1.5`, and there is no changelog. The surface has grown
  a lot; a version bump and release notes are worth doing before anyone
  consumes this.
