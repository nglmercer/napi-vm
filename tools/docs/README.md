# Documentation checks

Run the link and example checks from the repository root. They cover first-party
Markdown, including package READMEs, generated references, and scaffold docs.
Third-party `vendor/` and dependency documentation are excluded.

## Check links

```sh
npm run docs:check
node tools/docs/check.mjs --json
```

The checker resolves local files/directories and Markdown heading/explicit
anchors. It skips code fences and external URLs. It does not fetch external
websites or establish that every prose statement is correct. Tracked deletions
are skipped while references to deleted targets still fail.

## Execute examples

Build the current addon and JavaScript fixture packages first:

```sh
npm run build:all
npm run plugins:build:ts
npm run docs:test
npm run docs:test:rust
```

`docs:test` executes each `javascript verify` fence directly from Markdown in
an isolated Node child process with a timeout. Top-level `await` is wrapped in
an async function for CommonJS examples. Snippets use checkout-root paths and
contain assertions. The runner constructs a temporary JS-only packaged greeter
with its SDK/protocol dependencies outside the checkout, passes its directory
as `PLUGIN_DIRECTORY`, and removes it afterward. The hosting example therefore
cannot accidentally rely on workspace modules for its child runtime.

`docs:test:rust` also extracts `rust verify` blocks into a uniquely named temporary
Cargo integration target, then tests them with locked dependencies, no default
facade features, and the runtime feature under `ci-optimized`. The temporary
target is removed in `finally`; no separate copy of the snippets is maintained.

The ordinary production host still requires explicit package/runtime/service
setup; the test fixture is a check of the documented path, not hidden production
loading behavior. Unmarked snippets can be partial configurations or require
separate browsers/platform fixtures. Review their prerequisites manually.

CI checks links in the quality job and runs examples after building fresh
bindings and JavaScript fixture packages in the generated-bindings job.
