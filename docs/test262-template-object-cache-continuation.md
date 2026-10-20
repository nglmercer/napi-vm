# Realm-owned tagged template cache

Each parsed tagged-template node receives a stable site identity. AST clones,
function factory results, and bytecode retain that identity. Fresh eval and
Function-constructor parses receive distinct identities. Each executing realm
owns its mapping from site to frozen template object, without consulting guest
properties or using template text as a key.

Realm cache values participate in GC tracing, edge clearing, and iterative
teardown. The same compiled program in different realms receives different
template objects. Cycle collection preserves same-site identity while its realm
remains reachable.

Five cache regressions and two integrity tests pass. Applicable cases explicitly
exercise AST and bytecode; direct eval runs through its supported AST fallback.
Three frozen baseline guest probes fail. All 369 core tests, strict workspace
Clippy, and formatting pass.

The debug worker passes **46 / 48** tagged-template variants. The combined cache
and preceding integrity implementation gains **14 passes with zero losses**
against the pre-integrity worker. The two remaining cases concern constructor
precedence. This is a debug measurement, separate from the published full
release score. See the [evidence](../tools/test262/evidence/template-object-cache-2026-10-20/).
