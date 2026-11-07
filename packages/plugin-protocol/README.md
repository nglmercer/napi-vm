# @napi-vm/plugin-protocol

Data validation and RPC transport primitives for trusted process plugins.
This package does not execute guest JavaScript or launch a plugin.

## Use it

```javascript
import { wireValue, PluginError, FrameDecoder, encodeFrame } from '@napi-vm/plugin-protocol';
const message = wireValue({ name: 'Ana', count: 2 });
const encoded = encodeFrame(message);
const decoded = new FrameDecoder().push(encoded);
```

`Contract`, `validate`, and `assertContract` describe and check normalized
interfaces. `RpcPeer` manages bounded asynchronous requests, cancellation,
deadlines, and notifications. `PluginError` carries stable protocol codes and
schema-validated domain-error data. Prefer the SDK and host packages for ordinary
application authoring rather than implementing startup/lifecycle manually.

Wire values are finite, data-only JSON with valid Unicode-scalar strings.
Functions, getters, sparse arrays, cycles, symbols, arbitrary class instances,
and non-finite numbers are rejected. Use declared encodings for 64-bit integers,
bytes, and UTC datetimes. This wire format differs from the VM's UTF-16 value
boundary.

## Develop

From the repository root:

```sh
npm run plugins:build:ts
npm run plugins:test:node
npm run plugins:typecheck
```

[Protocol guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/protocol.md) ·
[Contract guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/contracts.md)
