# Optional-chain references continuation

AST and bytecode evaluation now carry short-circuit state through ordinary
member accesses and calls following an optional access. A real undefined value
returned by a getter or call remains distinct from a skipped chain. Parentheses
end short-circuit propagation while retaining successful method receivers.

Optional private reads and calls perform the normal private-brand check on
non-nullish receivers. Computed public hash-named keys remain public. Computed
undefined keys are no longer confused with the parser's optional-call marker.
Deletion propagates a skipped chain as true and deletes the final reference
without reading its getter.

Eight regressions require prepared bytecode and independently execute the AST.
They cover private fields/methods, receiver preservation, computed keys,
argument/call suppression, true undefined results, parenthesized boundaries,
continued calls, and deletion. All 368 core-library tests pass without exclusions.
Strict workspace Clippy and formatting pass after collapsing a new conditional.

The frozen predecessor worker c44699f2 passes 16,710 / 16,765 variants across
optional-chaining expressions, class expressions, and class statements. The final fresh release passes **16,722 / 16,765**, gaining **12 passes with
zero losses**. All 76 optional-chaining expression variants pass. The remaining
43 failures include 38 parse failures and five runtime failures in class tests.
There are zero skips, timeouts, crashes, or harness errors. Reports, comparison,
worker identity, and source patch are archived in
[optional-chain evidence](../tools/test262/evidence/optional-chain-references-2026-10-19/).
The latest full run separately measures a528b02 at 101,895 / 102,956; this
continuation and the subsequent public-hash-key change are outside that snapshot.
