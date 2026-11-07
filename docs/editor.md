# Editor and live-runtime integration

The project has a shared Rust language service, a standalone LSP executable,
a browser playground, and a Node live-metadata publisher. Static analysis does
not require executing the user's document.

## Use the standalone language server

```sh
cargo build --release --locked --no-default-features --bin napi-vm-lsp
```

Configure your editor to start `target/release/napi-vm-lsp` over standard input
and output. It uses LSP `Content-Length` framing. Standard output is reserved
for protocol messages; diagnostics about the server itself go to stderr.

The server advertises completion, hover, document symbols, definitions,
references, highlights, rename, signature help, inlay hints, formatting,
code actions, semantic tokens, and document diagnostics. Documents use full
text synchronization. Its implementation owns the supported details; it is
not a drop-in TypeScript compiler or tsserver replacement.

Use [the Zed extension](../zed-extension/README.md) for binary discovery,
download, and local development setup. The old Node launcher under `lsp/`
remains a legacy integration; the native executable is the primary path.

## Query analysis directly

```javascript verify
const { LanguageService } = require('./index.js');
const assert = require('node:assert/strict');
const service = new LanguageService();
const uri = 'file:///example.js';
const source = 'const answer = 42; answer;';
service.open(uri, source);
assert(Array.isArray(service.diagnostics(uri)));
assert(Array.isArray(service.complete(uri, Buffer.byteLength(source))));
assert.equal(service.close(uri), true);
```

`complete` and `hover` accept UTF-8 byte offsets. JavaScript string indexing and
LSP columns use UTF-16 units; convert them rather than passing indices through.
For a known JavaScript prefix, `Buffer.byteLength(source.slice(0, index), 'utf8')`
computes its byte length, provided the index is at the intended code-point
boundary. The LSP's position adapter handles line endings and surrogate-pair
clamping; browser bindings have their own conversion helper.

## Publish application metadata

`VmSession` from `runtime/session.cjs` attaches to a VM and publishes metadata
for exposed functions, registered modules/globals, and observed handler shapes.
It is separate from `AsyncSession`: one publishes editor metadata, the other
owns asynchronous guest execution.

```javascript
const { Vm } = require('./index.js');
const { VmSession } = require('./runtime/session.cjs');
const vm = new Vm();
const session = new VmSession({ workspace: process.cwd(), vm });
session.exposeFunction('greet', name => `Hello, ${name}`, {
  params: [{ name: 'name', typeName: 'string' }],
  returns: 'string'
});
session.start();
// Keep the application running while an editor consumes its metadata.
// During application shutdown:
session.stop();
vm.dispose();
```

Use `registerGlobal` for an explicit shape, `observeHandler` for bounded
observations, and `registerModule` for source. Metadata declarations describe
the host's API; they do not implement argument validation or access control.
Avoid publishing secrets as sample values. `stop()` tears down transport and
locator state; it does not replace VM disposal.

The running application publishes `<workspace>/.napi-vm/runtime.json`. Without
a valid live locator the server continues static analysis. See
[runtime metadata protocol](runtime-protocol.md) for authentication and framing.

## Test an integration

```sh
npm run lsp:test
npm run runtime:smoke
npm run lint:playground
```

The LSP tests include transport/lifecycle behavior. `runtime:smoke` exercises the
Node publisher. Browser execution is covered separately by the
[WASM build/test workflow](../playground/README.md).
