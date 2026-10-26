# Getter/setter decorator follow-up — 2026-10-26

Candidate v5 passes **32,596 / 32,596** broad pinned variants, with zero
losses against the field-decorator stage and **42 gains with zero losses**
against the latest complete-report projection. No exclusions, skipped variants,
timeouts, crashes, or harness errors occur. The 14 inherited-accessor regression
variants also pass separately. The latest complete score remains
**102,839 / 102,956** until another complete run confirms the focused gains.

Getter/setter decorators now preserve source-order overrides, arbitrary callable
replacement identities, public and private getter/setter pairs, private brands,
kind-specific access contexts, static/instance ordering, and initializer lifetime.
Object and array accessor descriptors retain original callable values instead
of cloning or renaming them. Value-free metadata records canonical accessor
kinds. Reads, writes, reflection, property caches, and Node addon serialization
respect that metadata. Exotic inherited reads delegate through the owning
prototype with the original receiver. Internal Error virtual properties retain
their existing storage semantics. Array deletion/truncation clear stale metadata.
Auto-accessor decorators remain the next implementation stage.

Validation: **408 core tests**, five worker tests, all-target Clippy with warnings
denied, root all-target checking, formatting, whitespace, and locked release
build pass. The standard six-variant timing slice passes. Differential regressions verify both AST and bytecode execution.
The proxy-accessor diagnostic passes; the 30,000-element array pop diagnostic
finishes in about **1.62 seconds** under its five-second deadline. Diagnostics
do not count as corpus gains. Exact reports, projections, comparisons, source
diffs, worker identity, and raw check logs are retained and hashes revalidated.

Rejected candidates remain archived: v1 loses companion setter reflection;
v2 (commit 249f1e8) fails cache and Node addon core checks; v3 passes 407 core
tests but loses 14 inherited-accessor variants in the broad run (32,582 passes,
14 failures, zero timeouts); v4 fixes inherited reads but delegates internal
Error properties incorrectly, failing two core tests. V5 fixes those virtual
properties and passes all 408 tests. The v4 source is v5 minus the single
has_stored_own_property arm for Error message/name/stack/code. The v3 broad run
finished with its frozen worker while subsequent v4 debug checks were running;
all 14 failures are semantic, with no timing losses. V5 broad ran without builds
or concurrent core tests. These failures are not hidden or relabeled as passes.
