# Ordinary-object write optimization checkpoint

This is work in progress toward full pinned Test262 conformance. **The current
engine changes are not fully validated and have no measured Test262 gains.**
The separate [timeout audit](test262-timeout-audit.md) covers the earlier clean
baseline and rejected RegExp/TypedArray experiment, not this implementation.

## Current changes

Ordinary own string-keyed data-property writes can avoid constructing temporary
property descriptor objects. New keys require a check that the ordinary
prototype chain has no interceptor. Accessors, proxies, symbols, boxed
primitives, mapped arguments, namespaces, and distinct receivers keep the
general write path.

Wide objects use the existing shape index for absent-key checks as well as
successful lookups. Managed arbitrary slot mutations invalidate the index;
value-only assignments and checked appends maintain it. Exposing the raw
RefCell permanently disables cached lookup for that object, since a host can
retain the reference and mutate the slots later.

The added unit and integration tests cover managed and raw host mutation,
deletion and reinsertion order, descriptors, inherited setters, coercion,
prototype traps, distinct receivers, symbols, and garbage collection.

## Validation and interruption

The optimized test executables built. The captured core-library run announced
368 tests and recorded **98 successful completions** before the session ended.
There is no final test summary or recorded Rust panic. The new integration
suite has no completed result. The full parallel suite was not restarted after
the user reported that the app closed. No timeout rerun or scaling measurement
has completed for this implementation. The benchmark script is preserved as
unexecuted work, not evidence of a speedup.

A separate Node launch problem was diagnosed without repeating the heavy tests:
Codex's inherited LD_LIBRARY_PATH selects its bundled libc++_shared.so, which
does not export the __hash_memory symbol required by the installed Node binary.
The Termux library exports the symbol. Running `node --version` in a child with
LD_LIBRARY_PATH removed succeeds with v24.18.0. This confirms the Node
subprocess problem; it does not establish the cause of the whole app closing.
Kernel/cgroup memory-kill counters were inaccessible, so memory pressure
remains a possibility rather than a proven cause.

The full-conformance goal is paused. Before resuming broad validation, isolate
tests with one test thread and resource limits, capture exit status, and keep
Node subprocesses from inheriting the conflicting library path. Do not treat
this checkpoint as a full Test262 pass or update the project-wide score.

[Captured validation and crash evidence](../tools/test262/evidence/dictionary-writes-checkpoint-2026-10-24/).
