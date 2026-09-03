/**
 * Node platform: the ready-made {@link HostPlatform} for Node and Bun hosts.
 *
 * This is the ONLY place (besides `node-filesystem.ts`, `miniaudio.ts` and
 * `native/trusted-modules.ts`) allowed to import `node:*`. The portable core
 * never sees these imports, so desktop/bundled hosts that never import this
 * module never pay for them — statically or at runtime.
 */

import * as nodeCryptoModule from "node:crypto";
import { createRequire } from "node:module";
import * as nodePath from "node:path";

import type { HostCrypto, HostPath, HostPlatform } from "../platform";
import {
  createNodeFileSystem,
  type NodeFileSystemOptions,
} from "./node-filesystem";

/** Native path helpers backed by `node:path`. */
const nodePathAdapter: HostPath = {
  sep: nodePath.sep,
  cwd: () => process.cwd(),
  resolve: (...parts) => nodePath.resolve(...parts),
  join: (...parts) => nodePath.join(...parts),
  normalize: (path) => nodePath.normalize(path),
  dirname: (path) => nodePath.dirname(path),
  basename: (path, ext) => (ext === undefined ? nodePath.basename(path) : nodePath.basename(path, ext)),
  extname: (path) => nodePath.extname(path),
  relative: (from, to) => nodePath.relative(from, to),
  isAbsolute: (path) => nodePath.isAbsolute(path),
};

/** Synchronous crypto backed by `node:crypto` (includes digests). */
export const nodeCrypto: HostCrypto = {
  randomBytes: (count) => new Uint8Array(nodeCryptoModule.randomBytes(count)),
  randomUUID: () => nodeCryptoModule.randomUUID(),
  digest: (algorithm, data) => nodeCryptoModule.createHash(algorithm).update(data).digest("hex"),
};

export interface NodePlatformOptions extends NodeFileSystemOptions {}

/**
 * Assemble the Node/Bun platform: `node:fs` backend with byte limits,
 * native paths, `node:crypto`, and a `require` rooted at the process working
 * directory for host-side native packages.
 */
export function nodePlatform(options: NodePlatformOptions = {}): HostPlatform {
  const cwdRequire = createRequire(`${process.cwd()}/package.json`);
  return {
    fs: createNodeFileSystem(options),
    path: nodePathAdapter,
    crypto: nodeCrypto,
    requireNative: (specifier) => cwdRequire(specifier),
  };
}
