# Static source-phase import grammar

Static `import source binding from 'module'` declarations now retain the source
phase in the AST. Contextual binding names `source` and `from`, line terminators,
and import attributes are supported. Ordinary `import source from 'module'`
remains an evaluation-phase default import. Namespace, named, and combined
clauses are not accepted after the source modifier.

The grammar follows the [source-phase imports proposal](https://tc39.es/proposal-source-phase-imports/).
The existing loader contract does not provide module-source objects. Linking a
static source request therefore reports a host TypeError before evaluating any
module body; the script embedding import path rejects it as well. This is an
explicit unsupported capability, rather than treating the request as a default
export import. Host-provided source objects, their intrinsic prototype,
re-exports, and source-phase graph loading remain required work.

Both new regressions fail before this change. Two source-phase tests, eight
byte-module tests, 33 module tests, strict workspace Clippy, and formatting pass.
No release Test262 gain is claimed. The active full worker freezes 75c6857 and
excludes this continuation.

The realm-owned AbstractModuleSource intrinsic is now installed without a global
binding and exposed to conformance tests through `$262.AbstractModuleSource`.
Its abstract call/construct paths throw TypeError; metadata, prototype links,
constructor back-reference, and getter-only Symbol.toStringTag descriptors are
installed. Ordinary receivers have no ModuleSourceClassName slot, so the getter
returns undefined. Each realm owns distinct constructor and prototype objects.
The public embedding accessor is `abstract_module_source_constructor()`.

All eight pinned AbstractModuleSource cases pass with a debug worker; this is
not a release score update. All 21 worker integration tests, strict workspace
Clippy, and formatting pass. Host source-object branding, source hooks, graph
loading, and re-exports remain required work.

All 369 core-library tests also pass without exclusions. Evidence is archived
in [intrinsic validation](../tools/test262/evidence/abstract-module-source-2026-10-20/summary.json).

Hosts can now create branded source objects using
`Interpreter::create_module_source_object(class_name)`. The class name is an
internal metadata slot, read only from the actual receiver by the intrinsic
tag getter. Inherited slots, proxies, and guest properties cannot forge it.
Objects inherit the creating realm’s AbstractModuleSource prototype; getters
from another realm can read the brand. Freezing preserves it. This API does
not yet connect source objects to module loading or re-exports.

All three source-phase tests and 369 core-library tests pass, with strict
workspace Clippy and formatting. See [brand validation evidence](../tools/test262/evidence/module-source-brand-2026-10-20/summary.json).
