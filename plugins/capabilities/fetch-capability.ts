/**
 * The standard guest `fetch()` capability: HTTP, against an explicit allowlist.
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
  defineCapability,
  isPermissionGranted,
  type CapabilityDefinition,
} from "./capability-registry";

const FETCH_GLOBALS = ["__cap_fetch", "fetch", "Headers", "Request", "Response"] as const;

/** An origin pattern: an exact origin, or `*` for any. */
export type FetchPermission = boolean | string | string[];

/** Default ceiling on a response body, so one reply cannot exhaust memory. */
export const DEFAULT_MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

const FETCH_GLOBAL_SOURCE = `
(() => {
  class Headers {
    constructor(init) {
      this._values = Object.create(null);
      this._isHeaders = true;
      if (init && init._isHeaders === true) {
        for (const name of Object.keys(init._values)) this.set(name, init._values[name]);
      } else if (Array.isArray(init)) {
        for (const pair of init) this.append(pair[0], pair[1]);
      } else if (init && typeof init === "object") {
        for (const name of Object.keys(init)) this.append(name, init[name]);
      }
    }
    append(name, value) {
      const key = String(name).toLowerCase();
      const text = String(value);
      this._values[key] = this._values[key] === undefined ? text : this._values[key] + ", " + text;
    }
    set(name, value) { this._values[String(name).toLowerCase()] = String(value); }
    get(name) {
      const key = String(name).toLowerCase();
      return this._values[key] === undefined ? null : this._values[key];
    }
    has(name) { return this.get(name) !== null; }
    forEach(callback, thisArg) {
      for (const name of Object.keys(this._values)) callback.call(thisArg, this._values[name], name, this);
    }
    entries() {
      const pairs = [];
      for (const name of Object.keys(this._values)) pairs.push([name, this._values[name]]);
      return pairs;
    }
    keys() { return Object.keys(this._values); }
    values() {
      const values = [];
      for (const name of Object.keys(this._values)) values.push(this._values[name]);
      return values;
    }
  }
  Headers.prototype.delete = function(name) {
    delete this._values[String(name).toLowerCase()];
  };

  class Request {
    constructor(input, init) {
      const options = init ?? {};
      const source = input && input._isRequest === true ? input : undefined;
      this._isRequest = true;
      this.url = source ? source.url : String(input);
      this.method = String(options.method ?? (source ? source.method : "GET")).toUpperCase();
      this.headers = new Headers(options.headers ?? (source ? source.headers : undefined));
      this.body = options.body ?? (source ? source.body : null);
      this.signal = options.signal ?? (source ? source.signal : undefined);
    }
  }

  class Response {
    constructor(body, init) {
      const options = init ?? {};
      this.body = body === null || body === undefined ? "" : String(body);
      this.status = options.status ?? 200;
      this.statusText = options.statusText ?? "";
      this.ok = this.status >= 200 && this.status < 300;
      this.url = options.url ?? "";
      this.headers = new Headers(options.headers);
      this.bodyUsed = false;
    }
    async text() {
      if (this.bodyUsed) throw new TypeError("Response body is already used");
      this.bodyUsed = true;
      return this.body;
    }
    async json() {
      if (this.bodyUsed) throw new TypeError("Response body is already used");
      this.bodyUsed = true;
      return JSON.parse(this.body);
    }
    clone() {
      if (this.bodyUsed) throw new TypeError("Response body is already used");
      return new Response(this.body, {
        status: this.status,
        statusText: this.statusText,
        headers: this.headers,
        url: this.url,
      });
    }
  }

  async function fetch(input, init) {
    const request = new Request(input, init);
    if (request.signal !== undefined && request.signal !== null) {
      throw new TypeError("AbortSignal is not supported by this fetch capability");
    }
    if (request.body !== null && typeof request.body !== "string") {
      throw new TypeError("This fetch capability accepts string request bodies");
    }
    const raw = await __cap_fetch(request.url, {
      method: request.method,
      headers: request.headers._values,
      body: request.body,
    });
    return new Response(raw.body, {
      status: raw.status,
      statusText: raw.statusText,
      headers: raw.headers,
      url: raw.url,
    });
  }

  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;
  globalThis.fetch = fetch;
})();
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
  // Validated eagerly, so a malformed origin fails at load time rather
  // than on the first request — the same rule the path patterns follow.
  // The raw value is stored; the definition compiles it again at load.
  validate(value, field) {
    compileFetchPermission(value, field);
    return value;
  },
  compile: (request) => compileFetchPermission(request),
  // Mirror the historic gate: an empty origins list installs nothing.
  allows: (request, grant) =>
    isPermissionGranted(grant) &&
    (request === true ||
      (typeof request === "string" && request !== "") ||
      (Array.isArray(request) && request.length > 0)),
  install: ({ vm, permissions, grant }) => {
    const policy = (grant !== null && typeof grant === "object" ? grant : {}) as FetchPolicy;
    // Sound cast: the `fetch` binding compiled these origins at load.
    installFetch(vm, permissions.fetch as CompiledFetchPermissions, policy);
    return () => {
      for (const name of FETCH_GLOBALS) vm.removeGlobal(name);
    };
  },
};

defineCapability(FETCH_CAPABILITY);

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

  vm.run(FETCH_GLOBAL_SOURCE);
}
