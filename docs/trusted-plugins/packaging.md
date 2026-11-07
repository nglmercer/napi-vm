# Build and package trusted plugins

Development, compilation, packaging, and loading are separate operations.
Normal host loading launches a declared built entry. It does not download
toolchains, install packages, or compile TypeScript/Rust.

## Manifest identity and launch

Trusted manifests use `manifestVersion: 2` and `execution: "trusted-process"`.
Required application fields include `id`, `version`, `protocol`, `provides`,
`requiresHost`, and `launch`. `contracts` lists normalized contract artifacts.
Optional fields include a profile, development entry, assets, extensions, and
native/service/capability dependencies. The machine-readable
[manifest schema](../../contracts/trusted-plugins/manifest.schema.json) and
runtime validator define the exact accepted fields.

| Profile | Intended artifact |
| --- | --- |
| `portable-js` | JavaScript source intended for declared supported runtimes |
| `external-runtime` | JavaScript entry requiring an external Node/Bun runtime |
| `native-executable` | Executable built for a declared OS/architecture/libc |

JavaScript launch fields include `kind`, `entry`, runtime list, optional
preference/version constraints, and arguments. Native launch uses
`kind: "executable"`, `entry`, arguments, and a target. Do not label a native
artifact portable merely because its protocol is portable.

## Build explicitly

For a generated TypeScript/JavaScript project, install its dependencies and run
these commands from that project:

```sh
npm run typecheck
npm run build
npm run test:node
```

The equivalent CLI build is `napi-vm-plugin build . --format js`. Rust and
Bun-compiled executable projects use `--format executable`; Rust requires
Cargo and the selected target toolchain, while Bun compilation requires Bun.
The build stages output and updates launch metadata to the actual artifact.
A rebuild invalidates an old inventory lock; pack again afterward.

## Create a relocatable package

```sh
napi-vm-plugin test . --artifact --runtime node
napi-vm-plugin pack . --out ../packaged-plugin
napi-vm-plugin validate ../packaged-plugin
```

Use a fresh output outside the source directory. Packaging copies the built
entry, required contracts/assets, and files selected by explicit include rules,
and writes `plugin.lock.json` with artifact target/runtime/ABI/availability,
interface identities, and file digests. Runtime dependency closure is the
project's responsibility: a source entry that imports missing modules is not
made standalone by adding a manifest.

Native artifact tests require a Rust host:

```sh
napi-vm-plugin test . --artifact \
  --host-executable /absolute/path/to/trusted-host-rust
```

Supply the executable for the host platform. A built artifact, a distributed
artifact, and a successfully tested artifact are distinct statuses. Keep them
separate in release metadata and compatibility reports.

The artifact test reads `plugin.test.json` from the directory passed to it. If
that directory has no lock, it creates a temporary packaged artifact for the
scenarios. To run scenarios directly from a distributed directory, include
`plugin.test.json` in the manifest's assets or explicit pack inclusion list
before packaging. It is not required for ordinary production loading.

## Verify distribution behavior

Test the package after relocation, including paths with spaces/non-ASCII
characters and a host environment without the source project's development
dependencies. Verify tampered bytes are rejected, startup requirements are
reported, and shutdown leaves no children running. Native dependencies must
match the actual runtime, ABI, OS, architecture, and libc.

Package SHA-256 inventories detect modification relative to recorded bytes.
They are not signatures or proof of publisher authorization. Choose a trusted
delivery/signing mechanism for your application's threat model.

See [testing](testing.md) for the repository's consumer and relocation checks.
