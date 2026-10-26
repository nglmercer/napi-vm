# Method decorator follow-up — 2026-10-26

This stage adds method decorators in both class frontends. The broad selection
passes **26,471 / 26,479** variants, with **zero lost passes** against the
latest complete-report projection. Its eight failures are the same
field-dependent element-decorator variants. The exact 16,882-variant class
projection has **zero gains and zero losses** against the preceding class
decorator worker. The 34 earlier focused gains against the full baseline are
retained; this method stage adds no pinned-corpus gains yet. Six explicitly
separate worker smoke probes pass, as does the six-variant timing slice.
No variants are skipped or excluded.

Method decorator expressions evaluate before their computed keys in the
class environment. Static decoration precedes instance decoration, with
reverse application within each decorator list. Ordered definitions survive
until installation, preserving overwritten method side effects and later
method/getter/setter/auto-accessor overrides. Public and private methods accept
callable replacements (including proxies) or undefined. Context access.has
and access.get preserve symbol keys, private identities, receiver checks,
and current values. Private replacements update the installed private slots.
Method initializers run before fields; static initializers run after class
decoration and before static fields. Internal state lives in the nearest
class environment and is traced through existing bindings. Unsupported field,
getter/setter, and auto-accessor decorators still produce parse errors.

Candidate v1 failed a differential truth assertion when a decorator returned
a getter-named function as an ordinary method. The source and failing log are
retained; that candidate did not build a release worker. Candidate v2 records
explicit data definitions independently of the legacy callable-name encoding.
Reads, descriptor reflection, and inherited writes respect those data tags;
accessor conversion and deletion clear them. A hash set avoids linear scans
as data definitions grow. Regression cases cover string/symbol keys, inherited
writes, deletion, freezing, and method identity. All **402 core tests**, five
worker tests, Clippy, root all-target checking, formatting, and whitespace
checks pass. The raw check log and exact source (including new decorators.rs)
are archived. Source and binary hashes were revalidated after measurement.

Next: field initializer transformations and per-field extras; getter/setter
and auto-accessor decoration; a shared context constructor with verified
property creation order; preservation of arbitrary callable accessor identity.
The eight residual pinned variants combine methods and fields, so they remain
nonpassing until the field work is implemented. The latest complete score
remains **102,839 / 102,956**, and the full-pass goal remains incomplete.
