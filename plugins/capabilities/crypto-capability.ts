/**
 * The `napi:crypto` capability: random bytes, UUIDs and digests.
 *
 * Randomness and hashing reach nothing outside the process and observe
 * nothing about it, so there is no path or origin to check — but the module
 * is still only registered when the manifest asks for it, because a
 * cryptographic source is a capability a host may want to withhold (a
 * deterministic replay harness, say, or a plugin that has no business
 * generating keys).
 */

import * as nodeCrypto from "node:crypto";

import { PermissionDeniedError } from "../core/errors";

import {
  unbindCapabilityModule,
  type CapabilityDefinition,
} from "./capability-registry";

const CRYPTO_GLOBALS = [
  "__cap_crypto_random_bytes",
  "__cap_crypto_random_uuid",
  "__cap_crypto_digest",
] as const;

export const CRYPTO_MODULE_NAME = "napi:crypto";

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

export function getRandomValues(target) {
  const bytes = __cap_crypto_random_bytes(target.length);
  for (let i = 0; i < target.length; i++) target[i] = bytes[i];
  return target;
}

export function randomUUID() {
  return __cap_crypto_random_uuid();
}

export function digest(algorithm, data) {
  return __cap_crypto_digest(algorithm, data);
}
`;

/**
 * Registry entry: no options (any options object is refused), teardown
 * returned to the host — no `uninstallCryptoCapability` to remember.
 */
export const CRYPTO_CAPABILITY: CapabilityDefinition = {
  name: "crypto",
  install: ({ vm }) => {
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
      return new Uint8Array(nodeCrypto.randomBytes(count));
    });

    vm.exposeFunction("__cap_crypto_random_uuid", () => nodeCrypto.randomUUID());

    vm.exposeFunction("__cap_crypto_digest", (algorithm: unknown, data: unknown) => {
      const name = String(algorithm).toLowerCase();
      if (!ALGORITHMS.has(name)) {
        throw new PermissionDeniedError(`unsupported digest algorithm: ${String(algorithm)}`);
      }
      const bytes =
        data instanceof Uint8Array
          ? data
          : new TextEncoder().encode(typeof data === "string" ? data : String(data));
      return nodeCrypto.createHash(name).update(bytes).digest("hex");
    });

    vm.registerModule(CRYPTO_MODULE_NAME, CRYPTO_MODULE_SOURCE);
    return () => unbindCapabilityModule(vm, CRYPTO_MODULE_NAME, CRYPTO_GLOBALS);
  },
};
