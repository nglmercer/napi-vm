import * as fs from "node:fs";

import { ResourceLimitError } from "../core/errors";

/**
 * The host-side filesystem backend.
 *
 * Everything the plugin host needs from the outside world goes through this
 * interface, so the same permission logic can sit on top of Node, Bun, Deno or
 * a future Rust implementation without the guest-facing API changing.
 *
 * All paths crossing this interface are *native, already-permitted* paths. No
 * implementation of this interface performs permission checks — that happened
 * before the call.
 */
export interface HostFileSystem {
  /**
   * Fully resolved real path (symlinks followed), or `null` when the path does
   * not exist. Any other failure must throw.
   */
  realpath(nativePath: string): string | null;
  readText(nativePath: string): string;
  writeText(nativePath: string, contents: string): void;
  exists(nativePath: string): boolean;
}

/**
 * Default cap on a single `readText`/`writeText`, in bytes.
 *
 * Permission to read a path is not permission to spend unbounded host memory
 * on it. The VM's own 16 MiB string ceiling only rejects the value *after*
 * Node has allocated the whole file, so a permitted 2 GiB file would still be
 * read, marshalled and only then refused. 8 MiB leaves headroom under that
 * ceiling while staying far above any plausible plugin source or config file.
 */
export const DEFAULT_MAX_FILE_BYTES = 8 * 1024 * 1024;

export interface NodeFileSystemOptions {
  /** Largest file `readText` will load. Defaults to 8 MiB. */
  maxReadBytes?: number;
  /** Largest payload `writeText` will accept. Defaults to 8 MiB. */
  maxWriteBytes?: number;
}

/**
 * `O_NOFOLLOW`, or 0 where the platform lacks it (Windows).
 *
 * The permission layer hands us a canonical path, so the final component is
 * never legitimately a symlink. Refusing to follow one at open time closes the
 * window between `realpath` and the open in which that component could be
 * swapped for a link pointing somewhere unauthorized. Intermediate directory
 * components remain racy; closing that needs `openat2(RESOLVE_BENEATH)`, which
 * `node:fs` does not expose.
 */
const O_NOFOLLOW = fs.constants.O_NOFOLLOW ?? 0;

/** Node/Bun/Deno-compatible backend built on `node:fs`. */
export function createNodeFileSystem(
  options: NodeFileSystemOptions = {},
): HostFileSystem {
  const maxReadBytes = options.maxReadBytes ?? DEFAULT_MAX_FILE_BYTES;
  const maxWriteBytes = options.maxWriteBytes ?? DEFAULT_MAX_FILE_BYTES;

  return {
    realpath(nativePath) {
      try {
        return fs.realpathSync(nativePath);
      } catch (error) {
        const code = (error as NodeJS.ErrnoException).code;
        if (code === "ENOENT" || code === "ENOTDIR") return null;
        throw error;
      }
    },

    readText(nativePath) {
      const fd = fs.openSync(nativePath, fs.constants.O_RDONLY | O_NOFOLLOW);
      try {
        // Size is checked against the same descriptor we read from, so the
        // file cannot be swapped for a larger one between check and read.
        const stat = fs.fstatSync(fd);
        if (!stat.isFile()) {
          // Character devices and FIFOs report size 0 and would otherwise read
          // forever, or block the event loop waiting for a writer.
          throw new ResourceLimitError("path is not a regular file");
        }
        if (stat.size > maxReadBytes) {
          throw new ResourceLimitError(
            `file is larger than the ${maxReadBytes} byte read limit`,
          );
        }

        const buffer = Buffer.allocUnsafe(stat.size);
        let filled = 0;
        while (filled < stat.size) {
          const read = fs.readSync(fd, buffer, filled, stat.size - filled, filled);
          if (read === 0) break;
          filled += read;
        }
        return buffer.subarray(0, filled).toString("utf8");
      } finally {
        fs.closeSync(fd);
      }
    },

    writeText(nativePath, contents) {
      const bytes = Buffer.byteLength(contents, "utf8");
      if (bytes > maxWriteBytes) {
        throw new ResourceLimitError(
          `contents are larger than the ${maxWriteBytes} byte write limit`,
        );
      }
      const fd = fs.openSync(
        nativePath,
        fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_TRUNC | O_NOFOLLOW,
        0o666,
      );
      try {
        fs.writeFileSync(fd, contents, "utf8");
      } finally {
        fs.closeSync(fd);
      }
    },

    exists(nativePath) {
      return fs.existsSync(nativePath);
    },
  };
}
