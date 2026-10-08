# Realm and buffer continuation evidence

Phases 1–3 remain incomplete and PR #24 remains draft. This evidence measures
commit `f078fcb` on the pinned agents/realms/gc-weak union, not the full corpus.

The 2,030 variants have 1,640 passes and 390 failures, with no harness errors,
timeouts, crashes, or skips. Against the identical `313146b` selection, the
comparison has 139 new passes and zero lost passes. Source configuration, worker
hash, required-check logs, and archive digests are in `f078fcb-summary.json`.
All required checks pass: 805 workspace tests (four existing ignored), 158 minimal
tests, Node, WASM, formatting, strict Clippy, and nine runner/tooling tests.
Two new verified-bytecode/AST buffer-growth fixtures agree. Full-corpus tier
mismatch measurement remains pending.

The earlier `df3d691` exploratory run is retained: 1,520 passes, 510 failures,
23 new passes and four lost passes. The losses were constructor post-return
errors allocated in the callee realm; `f078fcb` corrects them to the caller realm.
No exploratory outcomes were removed or edited.

Reproduce the focused comparison from the repository root:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-realms-memory/313146b-baseline-focused.json.gz \
  tools/test262/evidence/foundations-realms-memory/f078fcb-focused.json.gz \
  --output artifacts/test262/reproduced-realm-memory-transitions.json
```

The latest historical full-corpus evidence remains the Phase 2 `313146b` run;
its 50,926 passes must not be attributed to this source revision.
