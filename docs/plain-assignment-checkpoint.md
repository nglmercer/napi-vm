# Plain assignment reference checkpoint

Continues commit `9d7ce1b` (Iterator helpers and compound assignments).

Both execution tiers resolve plain-assignment binding references and evaluate member targets before the RHS, preserving the original reference when RHS code changes scope or objects. Resolution does not invoke property getters. Member key conversion and write validation occur after the RHS. This includes super references, private-field writes, anonymous function/class name inference, and parenthesized-target naming. Number constants have their required non-writable, non-enumerable, non-configurable descriptors.

Eight integration regressions cover both AST and verified bytecode. Focused Test262 results on revision `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`:

- Assignment directory: **653/850** passed. All 197 remaining failures are in `dstr` or `destructuring`.
- Compound assignments: **786/786** passed.
- Logical assignments: **132/132** passed.
- Combined: **1,571/1,768**, zero crashes, timeouts, skips, or harness errors.
- No former passing variant in this selection became nonpassing relative to the archived full abstract-equality run.

Evidence: `tools/test262/evidence/plain-assignment/results.json.gz`, engine SHA-256 `2e5ed99cae51c0d35a546226f682c487e24b7deb51d6580b1989fb3cb7400c62`. The worker used a debug build, 8 jobs, and a 5-second deadline. No full-corpus or broad workspace verification was run.

The next assignment family to address is destructuring: consume iterator records in order, close on abrupt completion, evaluate assignment targets at the specified point, and infer default initializer names.
