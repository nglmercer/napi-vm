# Plugins

`plugins/` is a capability-based plugin host built on top of the VM. It lives
entirely outside the Rust interpreter: the core knows nothing about manifests,
globs, plugin roots or filesystems.

```text
plugin.json
     │
     ▼
PluginHost ── validates manifest, compiles permissions, resolves paths,
     │        installs the granted capability modules, manages the lifecycle
     ▼
  napi-vm
     │
     ▼
 plugin.js
```

```bash
bun examples/plugins.ts
```

## Manifest

```json
{
  "name": "example-plugin",
  "version": "1.0.0",
  "apiVersion": 1,
  "entry": "./plugin.js",
  "permissions": {
    "fs": {
      "read": ["./config.json", "./assets/**"],
      "write": "./cache/**"
    },
    "path": true,
    "crypto": true,
    "timers": true,
    "fetch": ["https://api.example.com"]
  }
}
```

A manifest is validated when the plugin loads — name, version, `apiVersion`,
entry (which must stay inside the plugin directory) and every permission
pattern. Malformed patterns fail at load time, not on the first filesystem
call.

Each `permissions` key is validated by a schema registered for it
(`definePermissionSchema`) — the manifest module itself names no key, so new
capabilities add keys without touching it. An unknown key fails the load:
a typo'd permission is never silently ignored. The entry string is checked
for shape at parse time and normalized plus containment-checked by the host
(`validateEntryPath`, then `realpath`).

| Value | Meaning |
|-------|---------|
| missing / `false` | denied |
| `true` | same as `"*"` |
| `"*"` | requests unrestricted access |
| `"./assets/**"` | a plugin-relative subtree |
| `"/usr/share/app/**"` | an absolute path (host policy still decides) |
| `["./a", "./b/**"]` | any of several patterns |

Glob syntax is deliberately small: `*` matches within one path segment, `**`
matches zero or more segments. Everything else is literal.

## The manifest is a request, not a grant

```text
requested permissions  ∩  host policy  =  effective permissions
```

```ts
const host = new PluginHost({
  policy: {
    fs: {
      absoluteRead: false,   // may the plugin read outside its own root?
      absoluteWrite: false,  // may it write outside? dangerous
      deny: ["/etc/**"],     // always refused
      allow: ["/srv/data/**"], // when present, out-of-root paths must match
    },
  },
});
```

A plugin asking for `"read": "*"` gets unrestricted reads *inside its own
directory*; anything beyond that needs `absoluteRead`.

## Path resolution

`"./"` is always the plugin root — never `process.cwd()`. Every privileged call
resolves its path before checking anything:

```text
guest path → fold separators → resolve . and .. → resolve against plugin root
  → canonicalize (follow symlinks) → host policy → manifest permission → I/O
```

Because the check happens on the canonical path, `./cache/../../secret.txt` and
a `cache/outside -> /etc` symlink are both refused, and an in-root symlink is
matched by where it really points.

Guest paths are POSIX on every host; the host converts to native paths when it
performs I/O.

## Guest API

```js
import { readText, writeText, exists } from "napi:fs";
import { join, normalize, dirname, basename, extname } from "napi:path";
```

`napi:fs` is always registered — registering it grants nothing, since each
function checks its own path. `napi:path` is pure computation and is registered
only when the manifest asks for `"path": true`.

Denied calls raise a catchable error carrying no host paths:

```js
try {
  readText("./secret.txt");
} catch (error) {
  error.name;    // "PermissionDenied"
  error.message; // 'fs.read is not permitted for "./secret.txt"'
}
```

Guests never see `require`, `process`, `node:fs`, `Bun` or `Deno`.

## Entry module

The default export may be an object or a class; both are normalized to a single
instance.

```js
import { readText, writeText } from "napi:fs";
import { join } from "napi:path";

export default class ExamplePlugin {
  onLoad(context) {
    this.config = JSON.parse(readText("./config.json"));
    writeText(join("./cache", "status.json"), JSON.stringify({ plugin: context.name }));
  }

  onUnload(context) {
    return { config: this.config }; // serializable state, survives a reload
  }

  onReload(context, previousState) {
    this.config = previousState ? previousState.config : JSON.parse(readText("./config.json"));
  }
}
```

`context` carries `name` and `version` only — the plugin root is host-side
information and is deliberately withheld. `onUnload` also receives
`reason: "unload" | "reload"`. All three hooks are optional; a plugin without
`onReload` falls back to `onLoad`.

## Host API

```ts
const host = new PluginHost({ policy });

const plugin = host.load("./examples/plugins/example-plugin");
plugin.loadResult;      // whatever onLoad returned
plugin.status;          // "loaded" | "error"

host.reload("example-plugin"); // fresh VM, state handed to onReload
host.unload("example-plugin"); // returns onUnload's state
host.get("example-plugin");
host.list();
host.unloadAll();
```

Reload never mutates a half-loaded environment: the old instance gets
`onUnload({ reason: "reload" })`, its serializable return value is kept, the VM
is discarded, and everything is rebuilt from disk — so edited source *and*
edited permissions both take effect.

Unload calls `onUnload`, then detaches the capabilities: the modules and the
bridge globals are removed from the VM. A lifecycle hook that throws revokes
them immediately as well — an errored plugin never keeps a live `napi:fs` — and
a failed `onUnload` still unloads the plugin. The registry keeps the errored
entry so `reload()` can rebuild it from disk.

## Swapping the filesystem

Everything the host touches goes through one small interface, so the same
permission logic can sit on Node, Bun, Deno or a Rust backend without the
guest-facing API changing:

```ts
new PluginHost({
  policy,
  fs: { realpath, readText, writeText, exists },
});
```

### Size limits

Permission to read a path is not permission to spend unbounded host memory on
it. The default Node backend caps each read and write at 8 MiB and raises
`ResourceLimitError` (`e.name === "ResourceLimit"` in guest code) past that:

```ts
import { createNodeFileSystem, PluginHost } from "napi-vm/plugins";

new PluginHost({
  policy,
  fs: createNodeFileSystem({
    maxReadBytes: 1 * 1024 * 1024,
    maxWriteBytes: 256 * 1024,
  }),
});
```

The cap is deliberately separate from `PermissionDeniedError`: the path *was*
authorized, and reporting a size problem as a permission failure sends plugin
authors to their manifest instead of to the file. It is enforced against the
same descriptor the data is read from, so the file cannot be swapped for a
larger one between the check and the read, and non-regular files (devices,
FIFOs) are refused rather than read forever.

A custom `fs` implementation is responsible for its own limits — the VM's own
16 MiB string ceiling only rejects the value after the host has already
allocated it.

## Layout

```text
plugins/
  index.ts                  public surface (barrel, organized by layer)
  core/                     host engine
    plugin-host.ts          load / reload / unload
    manifest.ts             plugin.json types and validation
    permissions.ts          compilation, policy intersection, enforcement
    lifecycle.ts            guest-side bootstrap and hook wrappers
    errors.ts               error types
  capabilities/             guest modules
    capability-registry.ts  dynamic capability names (no hardcoded enum)
    filesystem-capability.ts  napi:fs
    path-capability.ts      napi:path
    crypto-capability.ts    napi:crypto
    timers-capability.ts    napi:timers
    fetch-capability.ts     napi:fetch
    audio-capability.ts     napi:audio via miniaudio_node (registry entry)
  native/                   npm/`.node` bridging (host-side, operator-gated)
    native-loader.ts        loaded code → guest module bridge
    trusted-modules.ts      pinned download + verify + require
  fs/                       filesystem + path support
    host-filesystem.ts      the swappable backend
    path-rules.ts           guest path normalization and glob matching
```

External code imports only the barrel (`../plugins`); internal
cross-imports are relative (`../core/errors`, `../fs/path-rules`), so files
can move between layers without touching consumers.

The capability installers use `exposeFunction` + `registerModule` so the host
runs against any published napi-vm build; `vm.registerHostModule()` (see the
[API reference](api.md)) is the newer core shortcut for the same shape.

Security regressions live in `tests/plugins/` — permissions, traversal,
symlink escapes, policy intersection, lifecycle and reload.

## The capability modules

| Module | Grants | Manifest | Host policy |
|--------|--------|----------|-------------|
| `napi:fs` | Reading and writing inside the granted patterns | `fs.read`, `fs.write` | `fs.absoluteRead`, `fs.absoluteWrite`, `fs.deny`, `fs.allow` |
| `napi:path` | POSIX path manipulation (no I/O) | `path: true` | — |
| `napi:crypto` | Random bytes, UUIDs, digests | `crypto: true` | `crypto: true` |
| `napi:timers` | The host clock | `timers: true` | `timers: true` or `{ resolutionMs }` |
| `napi:fetch` | HTTP to named origins | `fetch: [...]` | `fetch: { allow, deny, ... }` |
| `napi:audio` | Native playback (`miniaudio_node`) | `capabilities: { audio: true }` | `capabilities: { audio: true }` |

Every one of them is installed only when the manifest asks *and* the host
policy permits. Neither side can widen the other, and the default policy
grants none of them: a host that wants the network, the clock or a
cryptographic source says so.

## Dynamic capabilities

Beyond the built-ins, a plugin requests `capabilities: { "<name>": true }`
(or `{ "<name>": { ...options } }`). Names resolve against a host-side
registry — there is no manifest enum to extend:

```json
{ "permissions": { "capabilities": { "audio": true } } }
```

```ts
const host = new PluginHost({
  policy: { fs: { absoluteRead: false, absoluteWrite: false },
            capabilities: { audio: true } },
});
host.defineCapability(myCapability);   // trusted-operator API
host.setCapabilityEnabled("audio", false); // runtime kill-switch
```

Request ∩ policy ∩ runtime switch = installed. Unknown names fail the load
(a typo never becomes a silent grant); requested-but-ungranted names stay
absent. `napi:audio` is the first registry entry — playback through
`miniaudio_node`, with every `loadFile` path resolved through the plugin's
own `fs.read` permission first.

### Authoring a capability (subplugin shape)

Every capability — built-in or custom — is one `CapabilityDefinition`:
a `name`, an option `schema` with defaults, and an `install` that returns
its own teardown. There is no per-capability uninstall function; the host
runs the returned teardowns in reverse install order on unload, reload and
failed hooks:

```ts
import { defineCapability, unbindCapabilityModule } from "napi-vm/plugins";

defineCapability({
  name: "greet",
  schema: { voice: { type: "string", default: "alto", enum: ["alto", "bass"] } },
  install: ({ vm, options }) => {
    // Already schema-validated with defaults by the host; the cast only
    // recovers the static type.
    const { voice } = options as { voice: string };
    const globals = vm.registerHostModule("napi:greet", {
      hello: (name) => `${voice}: hi ${name}`,
    });
    return () => unbindCapabilityModule(vm, "napi:greet", globals);
  },
});
```

Schema rules: `true` in the manifest means "defaults" (`{}` when no schema
exists); unknown or mistyped options fail the load; numeric `min`/`max`,
`integer`, string `enum` and string arrays are enforced. A definition
without a schema takes no options at all. Policy-side extras (clock
precision, fetch allowlists, player factories) arrive via the `grant`, never
the manifest — guest-requested privilege would let the plugin choose its
own limits.

`napi:fs` and `napi:path` are the exception: they are substrate, not
registry entries — installed unconditionally (`fs`) or by boolean flag
(`path`) — because the permission checker itself stands on them.

## Trusted native packages

Downloading and exposing an npm / `.node` package is an operator action,
never a guest one. The native code is `require`d on the host; the VM only
ever sees wrapped functions through `registerHostModule`:

```ts
import { installTrustedPackage, nativePackageCapability } from "napi-vm/plugins";

const loaded = await installTrustedPackage(
  { dir: ".napi-vm/modules", allow: ["miniaudio_node"] },
  { package: "miniaudio_node", version: "1.6.3",
    integrity: "sha512-…" },   // pinned, verified before extraction
);
nativePackageCapability({ exposeAs: "audio", loaded, definition: {...} });
```

Fail-closed rules: exact pinned versions only (no ranges, no `latest`),
integrity verified with `timingSafeEqual` before anything is written,
tarball entries that escape their directory refused, and anything outside
the `allow` list is refused before any network happens. Verified installs
are cached on disk (`.verified.json`), so repeat loads need no network.

### `napi:crypto`

`randomBytes`, `getRandomValues`, `randomUUID` and `digest`. Nothing here
reaches outside the process or observes anything about it, so there is no path
or origin to check — but it is still a capability, because a host may want to
withhold a cryptographic source (a deterministic replay harness, or a plugin
that has no business generating keys). One `randomBytes` call is capped, so a
plugin cannot ask for a gigabyte of entropy.

### `napi:timers`

`now()`, `monotonic()` and `since(start)`. The VM's own `setTimeout` has no
wall clock — it orders callbacks without letting guest code observe or wait on
real time — and this capability is the opposite choice, granted explicitly. A
host that grants time at all can still deny *precise* time:

```ts
policy: { timers: { resolutionMs: 100 } }
```

rounds the clock down before the guest sees it, which is what makes timing
side channels expensive to use.

### `napi:fetch`

The capability that actually reaches outside the machine, so its checks are
the ones that matter:

- A manifest entry grants an **origin**, not a path. `"https://api.example.com/v1"`
  grants the origin; a same-origin path restriction is not a boundary a client
  can enforce, so it is not offered.
- Effective permission is requested ∩ policy, with the policy's `deny` checked
  first. A policy with no `allow` list permits nothing — the capability has to
  be opened deliberately, not by omission.
- Redirects are followed by hand and **each hop is re-checked**, so a permitted
  origin cannot bounce a request to a denied one.
- Only `http:` and `https:`; the response body is capped.

`napi:fetch` performs an async host call, which parks the VM thread, so it
runs under `runAsync`:

```ts
await vm.runAsync(`
  import { fetch } from "napi:fetch";
  const response = await fetch("https://api.example.com/items");
  response.json();
`);
```
