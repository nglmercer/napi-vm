# Your JavaScript process plugin

This project implements a trusted greeter interface using JavaScript and
checked-in contract metadata. It runs as a normal Node/Bun child process with
the launching account's privileges.

## Install and verify

Run from this generated project:

```sh
npm install
npm run typecheck
npm test
npm run build
npm run test:artifact
```

Keep business handlers in `src/plugin.js` and process setup in `src/main.js`.
`src/generated/greeter.js` carries the matching contract; source definitions live
under `contracts/`. Regenerate contract artifacts explicitly when changing the
interface. JavaScript runtime validation remains required even with typechecking.

`plugin.json` declares the built entry and runtimes. `plugin.build.json` controls
compilation; `plugin.test.json` defines artifact scenarios. `npm run dev` selects
an explicit build/watch/reload path.

## Distribute

```sh
npx napi-vm-plugin pack . --out ../packaged-plugin
npx napi-vm-plugin validate ../packaged-plugin
```

Package hashes identify bytes, not publisher trust. Test the relocated artifact
and shut down host/plugin resources. See the
[packaging guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/packaging.md).
