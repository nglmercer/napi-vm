# Source-phase module imports

Static `import source binding from 'module'` declarations retain the source
phase and attributes. Contextual names `source` and `from` and line terminators
are supported. Ordinary default imports named source remain evaluation-phase
imports. Namespace, named, and combined clauses are not accepted after the
source modifier.

Hosts implement `ModuleLoader::load_source_object(vm, id, attributes)` and return
the canonical identity and a branded object created through
`Interpreter::create_module_source_object(class_name)`. Unsupported hosts reject
by default. The object inherits the realm-owned AbstractModuleSource prototype;
its internal class-name slot cannot be forged through properties, inheritance,
or proxies. The abstract constructor throws on call and construction and is an
intrinsic rather than a global JavaScript binding. Embedders access it through
`abstract_module_source_constructor()`.

The engine caches source objects by canonical resource and normalized attributes.
Static imports, dynamic `import.source`, local re-exports, namespace access, and
star re-exports share identity. A source request does not load evaluation text,
link the target’s evaluation dependencies, or execute a module body. A later
ordinary import can load and evaluate the same resource while retaining the
source object. Source bindings are immutable. Hosts constructing `Module` records directly
initialize the new `source_object` field to None unless providing a source value. Host identity/brand violations
reject and are not cached. Cached source objects are traced by GC and released
when their module records are removed. Composite and TypeScript loaders forward
the source hook; other hosts explicitly opt in.

The Test262 host supplies the reserved `<module source>` representation. Other
source requests reject when no host representation exists. This follows the
[source-phase imports proposal](https://tc39.es/proposal-source-phase-imports/).

Validation covers grammar, source-only loading, shared static/dynamic/re-export
identity, ordinary evaluation after source loading, immutable bindings, invalid
host results and retry, cross-realm branding, and GC lifetime. Eight source tests,
eight byte-module tests, 33 module tests, 22 GC tests, 21 worker tests, and 369
core-library tests pass. Strict workspace Clippy, all-feature runtime Clippy, and formatting pass.
The final release passes **2,645 / 2,645** selected variants, gaining **13
passes with zero losses** versus 75c6857. These combined gains include the
preceding grammar, intrinsic, and brand checkpoints; they are not added to the
completed full score. Preliminary error-type regressions were found and fixed,
and source edges now do not block deferred synchronous readiness. See
[final source-loader evidence](../tools/test262/evidence/source-loader-2026-10-20/summary.json). The completed full snapshot 75c6857 excludes this
continuation.

Earlier grammar and intrinsic checkpoints remain archived in
[grammar evidence](../tools/test262/evidence/static-source-imports-2026-10-20/summary.json),
[intrinsic evidence](../tools/test262/evidence/abstract-module-source-2026-10-20/summary.json),
and [brand evidence](../tools/test262/evidence/module-source-brand-2026-10-20/summary.json).
