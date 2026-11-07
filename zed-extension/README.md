# napi-vm LSP

A thin launcher for the standalone native language server:

```
Zed extension
  ↓ locate (settings → $PATH) or download the platform binary
napi-vm-lsp
  ↓
napi_vm_core::lang::LanguageService (Rust core)
```

Node.js is never involved. Nothing is resolved out of the consumer project's
`node_modules`, so the extension behaves identically for npm, pnpm, Electron,
Electron ASAR, Bun-compiled applications, and projects with no `node_modules`
at all.

## How the binary is resolved

1. **Explicitly configured path** — `lsp.napi-vm.binary.path` in Zed settings:

   ```json
   {
     "lsp": {
       "napi-vm": {
         "binary": { "path": "/absolute/path/to/napi-vm-lsp" }
       }
     }
   }
   ```

2. **`$PATH`** — a `napi-vm-lsp` visible to the worktree's shell environment.

3. **GitHub Releases** — the platform archive for the latest release
   (`napi-vm-lsp-<os>-<arch>.tar.gz` / `.zip`), extracted into the extension's
   working directory and marked executable. Older versions are pruned.

The extension uses the normal platform archive when downloading automatically.
To select the musl archive on Linux, install it separately and configure its path.

## Local development

```bash
cargo build --release --no-default-features --bin napi-vm-lsp
export PATH="$PWD/target/release:$PATH"
```

Then install the `zed-extension/` directory through Zed's dev-extension flow.
Rebuild the extension itself with:

```bash
npm run zed:build   # cargo build --target wasm32-wasip2 --release
```

## Runtime metadata

The language server owns editor intelligence; the application owns runtime
metadata. They meet only at `<workspace>/.napi-vm/runtime.json` plus a Unix
domain socket (Windows: named pipe) — Runtime Protocol v1 — so neither side
depends on the other's filesystem layout.

Start a session to publish live host functions, module exports and observed
event shapes:

```bash
NAPI_VM_SESSION=1 bun examples/hotreload.ts
```

Without a running session no locator exists and the server simply serves
static analysis. No `.napi-vm.json` file is created or read by default.

## Linux compatibility

Releases ship both a glibc build (`napi-vm-lsp-linux-<arch>.tar.gz`) and a
statically linked musl build (`napi-vm-lsp-linux-<arch>-musl.tar.gz`). If the
glibc build reports `GLIBC_x.xx not found` on an older distribution, download
the musl archive and point `lsp.napi-vm.binary.path` at it.

JavaScript keeps Zed's built-in language (and its Tree-sitter highlighting);
this extension only contributes the napi-vm language server.
