# Clean full Test262 run — 2026-10-26

The complete pinned run on clean commit `9d23d087d0f7752853c24390776e00a605c28016`
passes **102,906 / 102,956 (99.9514%)**, with **4 failures and 46 timeouts**;
there are zero skips, crashes, or harness errors. Against the prior full run,
71 variants gain passes and four previously passing variants fail or time out.
Two lost passes are strict TypedArray prototype-chain assignments; two are
Proxy ownKeys timing cases. Both regressions are retained in the comparison.

The corpus is pinned to `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, with two
workers, five-second deadlines, no exclusions, and no resumed results. The
worker SHA-256 is `af0b2e63e48fa82c44ecb4eb4f956e203194dc07a84e54934ba4ca508d1865da`.
Source cleanliness, commit, corpus revision, and worker identity were checked.
The complete report, comparison, residual list, and source identity are here;
the per-file checkpoint remains under artifacts/test262.
