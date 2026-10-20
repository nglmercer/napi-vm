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
