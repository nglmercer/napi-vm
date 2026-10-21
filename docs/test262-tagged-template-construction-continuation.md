# Tagged template constructor precedence

The callee of a new expression now accepts tagged templates as MemberExpressions,
sharing the parser helper used for ordinary tagged-template postfix expressions.
The tag is invoked first; its result is then constructed. Constructor arguments
following the template are passed to that result, rather than to the tag.

Member access after tagging and repeated tagged applications remain part of the
callee expression. Tag member receivers are preserved, and the constructor sees
its own new.target. Template-site identity is allocated by the shared helper.

All three native regressions fail before the change. They now pass in AST and
prepared bytecode execution, along with five cache and two integrity tests.
Final core, Clippy, and conformance checks are recorded in the evidence archive.
Release verification remains separate from debug checks and the full score.

Template lookup now precedes cooked/raw array allocation and substitutions in both
tiers. Cache hits skip array construction but still evaluate tag getters and
substitutions. A verifier-checked LoadTemplate instruction implements that path.

All 369 core tests, 11 integration tests, strict workspace Clippy, and formatting
pass. The final debug worker passes **166 / 166** neighboring new/tagged-template
variants, including **48 / 48** tagged-template cases. The combined integrity,
cache, and precedence work gains 16 passes with zero losses versus the original
pre-template worker. Release verification of this latest patch remains pending.

The initial tag-only run had one tail-call timeout. A neighboring selection
passed before the lookup optimization and the final selection passes afterward;
no timing cause is inferred. All these reports are retained in the
[evidence archive](../tools/test262/evidence/tagged-template-construction-2026-10-20/).
