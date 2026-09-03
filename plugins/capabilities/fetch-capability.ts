/**
 * The `napi:fetch` capability: HTTP, against an explicit allowlist.
 *
 * This is the capability that actually reaches outside the machine, so it is
 * the one whose checks matter. Every request is matched against the plugin's
 * requested origins *and* the host policy before a socket is opened, on the
 * URL as parsed — never on the raw guest string — and redirects are followed
 * only to origins that pass the same check.
 */

import { PermissionDeniedError, PluginManifestError } from "../core/errors";
import type { Vm } from "../../index";

import {
  unbindCapabilityModule,
  type CapabilityDefinition,
} from "./capability-registry";

const FETCH_GLOBALS = ["__cap_fetch"] as const;

export const FETCH_MODULE_NAME = "napi:fetch";

/** An origin pattern: an exact origin, or `*` for any. */
export type FetchPermission = boolean | string | string[];

/** Default ceiling on a response body, so one reply cannot exhaust memory. */
export const DEFAULT_MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

const FETCH_MODULE_SOURCE = `
export async function fetch(url, options) {
  const response = await __cap_fetch(url, options ?? {});
  return {
    ok: response.ok,
    status: response.status,
    statusText: response.statusText,
    url: response.url,
    headers: response.headers,
    text() { return response.body; },
    json() { return JSON.parse(response.body); },
  };
}
`;

export interface FetchPolicy {
  /** Origins the host permits at all. `undefined` means "none". */
  allow?: string[];
  /** Origins always denied, checked before `allow`. */
  deny?: string[];
  maxResponseBytes?: number;
  /** How many redirects to follow. Each hop is re-checked. */
  maxRedirects?: number;
  timeoutMs?: number;
}

export interface CompiledFetchPermissions {
  origins: string[];
  any: boolean;
}

/**
 * Validate a manifest's `fetch` request into a list of origins.
 *
 * Each entry must be a parseable absolute URL; its *origin* is what is kept,
 * so `"https://api.example.com/v1"` grants the origin, not the path. Path
 * scoping is deliberately not offered: a same-origin path restriction is not
 * a security boundary a client can enforce.
 */
export function compileFetchPermission(
  value: unknown,
  field = "permissions.fetch",
): CompiledFetchPermissions {
  if (value === undefined || value === false) return { origins: [], any: false };
  if (value === true || value === "*") return { origins: [], any: true };
  const patterns = Array.isArray(value) ? value : [value];
  const origins: string[] = [];
  for (const entry of patterns) {
    if (typeof entry !== "string") {
      throw new PluginManifestError(`${field} entries must be strings`);
    }
    if (entry === "*") return { origins: [], any: true };
    let parsed: URL;
    try {
      parsed = new URL(entry);
    } catch {
      throw new PluginManifestError(`${field} entry is not a URL: ${entry}`);
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      throw new PluginManifestError(`${field} entry must be http or https: ${entry}`);
    }
    origins.push(parsed.origin);
  }
  return { origins, any: false };
}

/**
 * Decide whether `url` may be requested.
 *
 * Effective permission = requested ∩ host policy, with the policy's `deny`
 * checked first so it cannot be widened by either side.
 */
export function checkFetchOrigin(
  url: URL,
  requested: CompiledFetchPermissions,
  policy: FetchPolicy,
): void {
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new PermissionDeniedError(`fetch: unsupported protocol ${url.protocol}`);
  }
  const origin = url.origin;
  if (policy.deny?.includes(origin)) {
    throw new PermissionDeniedError(`fetch: ${origin} is denied by host policy`);
  }
  // A host that names no allowlist permits nothing: the capability has to be
  // opened deliberately, not by omission.
  const allowed = policy.allow ?? [];
  if (!allowed.includes("*") && !allowed.includes(origin)) {
    throw new PermissionDeniedError(`fetch: ${origin} is not permitted by host policy`);
  }
  if (!requested.any && !requested.origins.includes(origin)) {
    throw new PermissionDeniedError(`fetch: ${origin} is not in the plugin's manifest`);
  }
}

/// The one call shape the capability makes: a URL and a request init.
export type FetchTransport = (
  url: string,
  init?: RequestInit,
) => Promise<Response>;

/**
 * Registry entry. Requested origins arrive compiled in `permissions.fetch`
 * (malformed origins already failed the load); the host *grant* carries the
 * policy. The transport is always the global `fetch` — tests stub it —
 * because letting either side inject an HTTP client would move the trust
 * boundary into an invisible parameter.
 */
export const FETCH_CAPABILITY: CapabilityDefinition = {
  name: "fetch",
  install: ({ vm, permissions, grant }) => {
    const policy = (grant !== null && typeof grant === "object" ? grant : {}) as FetchPolicy;
    installFetch(vm, permissions.fetch, policy);
    return () => unbindCapabilityModule(vm, FETCH_MODULE_NAME, FETCH_GLOBALS);
  },
};

/** Implementation shared by the definition above; unexported on purpose. */
function installFetch(
  vm: Vm,
  requested: CompiledFetchPermissions,
  policy: FetchPolicy,
): void {
  const transport: FetchTransport = globalThis.fetch;
  const maxBytes = policy.maxResponseBytes ?? DEFAULT_MAX_RESPONSE_BYTES;
  const maxRedirects = policy.maxRedirects ?? 3;

  vm.exposeAsyncFunction("__cap_fetch", async (rawUrl: unknown, rawOptions: unknown) => {
    let url: URL;
    try {
      url = new URL(String(rawUrl));
    } catch {
      throw new PermissionDeniedError(`fetch: not a valid URL: ${String(rawUrl)}`);
    }
    checkFetchOrigin(url, requested, policy);

    const request = (rawOptions ?? {}) as Record<string, unknown>;
    const method = typeof request.method === "string" ? request.method.toUpperCase() : "GET";
    const headers =
      typeof request.headers === "object" && request.headers !== null
        ? (request.headers as Record<string, string>)
        : undefined;
    const body = typeof request.body === "string" ? request.body : undefined;

    // Redirects are followed by hand so each hop is checked; handing the
    // transport `redirect: "follow"` would let one permitted origin bounce the
    // request to a denied one.
    let current = url;
    let response: Response | undefined;
    for (let hop = 0; hop <= maxRedirects; hop++) {
      response = await transport(current.toString(), {
        method,
        headers,
        body,
        redirect: "manual",
        signal: policy.timeoutMs ? AbortSignal.timeout(policy.timeoutMs) : undefined,
      });
      const location = response.headers.get("location");
      if (response.status < 300 || response.status >= 400 || location === null) break;
      if (hop === maxRedirects) {
        throw new PermissionDeniedError("fetch: too many redirects");
      }
      current = new URL(location, current);
      checkFetchOrigin(current, requested, policy);
    }
    if (response === undefined) {
      throw new PermissionDeniedError("fetch: no response");
    }

    const text = await response.text();
    if (text.length > maxBytes) {
      throw new PermissionDeniedError(
        `fetch: response exceeds the ${maxBytes}-byte limit`,
      );
    }
    const headerEntries: Record<string, string> = {};
    response.headers.forEach((value, key) => {
      headerEntries[key] = value;
    });
    return {
      ok: response.ok,
      status: response.status,
      statusText: response.statusText,
      url: current.toString(),
      headers: headerEntries,
      body: text,
    };
  });

  vm.registerModule(FETCH_MODULE_NAME, FETCH_MODULE_SOURCE);
}
