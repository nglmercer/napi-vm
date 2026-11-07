# Your TypeScript process plugin

This project implements a trusted greeter interface. It runs as an ordinary
Node/Bun child process with your account's OS privileges.

## Install, test, and build

Run from this generated project:

```sh
npm install
npm run typecheck
npm test
npm run build
npm run test:artifact
```

`src/plugin.ts` contains handlers; `src/main.ts` starts the SDK session.
`src/generated/greeter.ts` and `contracts/greeter.contract.json` are generated
from the source descriptor/schema under `contracts/`. Change those inputs and
run explicit code generation when the interface changes.

`plugin.json` selects the built JavaScript entry. `plugin.build.json` configures
compilation, and `plugin.test.json` declares artifact scenarios. `npm run dev`
is an explicit compiler/watch/reload operation, not normal production loading.

## Package

```sh
npx napi-vm-plugin pack . --out ../packaged-plugin
npx napi-vm-plugin validate ../packaged-plugin
```

Test the relocated package in the actual target runtime. Always shut down a host
in `finally`; cooperative cancellation does not roll back external effects.
See the [authoring guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/authoring.md).
