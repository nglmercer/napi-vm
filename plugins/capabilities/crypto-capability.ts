/**
 * The `node:crypto` compatibility facade: random bytes, UUIDs and hashes.
 *
 * Randomness and hashing reach nothing outside the process and observe
 * nothing about it, so there is no path or origin to check — but the module
 * is still only registered when the manifest asks for it, because a
 * cryptographic source is a capability a host may want to withhold (a
 * deterministic replay harness, say, or a plugin that has no business
 * generating keys).
 *
 * The actual primitives come from the host platform (`platform.crypto`): the
 * Node platform backs them with `node:crypto`, portable hosts with WebCrypto.
 */

import { PermissionDeniedError } from "../core/errors";

import {
  booleanPermissionValue,
  defineCapability,
  unbindCapabilityModule,
  type CapabilityDefinition,
} from "./capability-registry";

const CRYPTO_GLOBALS = [
  "__cap_crypto_random_bytes",
  "__cap_crypto_random_uuid",
  "__cap_crypto_digest",
] as const;

export const CRYPTO_MODULE_NAME = "node:crypto";

/** Digest algorithms the capability will compute. */
const ALGORITHMS = new Set(["sha256", "sha384", "sha512", "sha1", "md5"]);

/**
 * The most bytes one `randomBytes` call will produce. A plugin asking for a
 * gigabyte of entropy is a denial-of-service attempt, not a use case.
 */
export const MAX_RANDOM_BYTES = 65_536;

const CRYPTO_MODULE_SOURCE = `
export function randomBytes(size) {
  return __cap_crypto_random_bytes(size);
}

export function randomUUID() {
  return __cap_crypto_random_uuid();
}

export function createHash(algorithm) {
  const chunks = [];
  return {
    update(data, encoding) {
      if (typeof data !== "string" && !(data instanceof Uint8Array)) {
        throw new TypeError("Hash.update(data) expects a string or Uint8Array");
      }
      if (typeof data === "string" && encoding !== undefined && encoding !== "utf8" && encoding !== "utf-8") {
        throw new TypeError("the sandboxed node:crypto facade supports UTF-8 text only");
      }
      chunks.push(data);
      return this;
    },
    digest(encoding) {
      if (encoding !== "hex") {
        throw new TypeError("the sandboxed node:crypto facade supports hex digests only");
      }
      return __cap_crypto_digest(algorithm, chunks);
    },
  };
}
`;

/**
 * Registry entry: no options (any options object is refused), teardown
 * returned to the host — no `uninstallCryptoCapability` to remember.
 */
export const CRYPTO_CAPABILITY: CapabilityDefinition = {
  name: "crypto",
  validate: booleanPermissionValue,
  install: ({ vm, platform }) => {
    const crypto = platform.crypto;
    vm.exposeFunction("__cap_crypto_random_bytes", (size: unknown) => {
      const count = Number(size);
      if (!Number.isInteger(count) || count < 0) {
        throw new PermissionDeniedError("randomBytes needs a non-negative integer size");
      }
      if (count > MAX_RANDOM_BYTES) {
        throw new PermissionDeniedError(
          `randomBytes is limited to ${MAX_RANDOM_BYTES} bytes per call`,
        );
      }
      return crypto.randomBytes(count);
    });

    vm.exposeFunction("__cap_crypto_random_uuid", () => crypto.randomUUID());

    vm.exposeFunction("__cap_crypto_digest", (algorithm: unknown, data: unknown) => {
      const name = String(algorithm).toLowerCase();
      if (!ALGORITHMS.has(name)) {
        throw new PermissionDeniedError(`unsupported digest algorithm: ${String(algorithm)}`);
      }
      const chunks = Array.isArray(data) ? data : [data];
      const encoded = chunks.map((chunk) => {
        if (chunk instanceof Uint8Array) return chunk;
        if (typeof chunk === "string") return new TextEncoder().encode(chunk);
        throw new TypeError("Hash.update(data) expects a string or Uint8Array");
      });
      const bytes = new Uint8Array(encoded.reduce((size, chunk) => size + chunk.length, 0));
      let offset = 0;
      for (const chunk of encoded) {
        bytes.set(chunk, offset);
        offset += chunk.length;
      }
      return crypto.digest(name, bytes);
    });

    vm.registerModule(CRYPTO_MODULE_NAME, CRYPTO_MODULE_SOURCE);
    return () => unbindCapabilityModule(vm, CRYPTO_MODULE_NAME, CRYPTO_GLOBALS);
  },
};

defineCapability(CRYPTO_CAPABILITY);
