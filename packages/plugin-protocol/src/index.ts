import { EventEmitter } from 'node:events';
import { createHash } from 'node:crypto';
import type { Socket } from 'node:net';

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };
export interface Schema { [key: string]: unknown }
export interface InterfaceDescriptor {
  descriptorVersion: number; id: string; version: string; types?: string;
  methods: Record<string, { input: string; output: string; errors?: readonly string[]; idempotent?: boolean }>;
  errors: Record<string, { data: string }>;
  events: Record<string, { payload: string }>;
  state?: { contract: string; version: number; schema: string };
}
export interface Contract { descriptor: InterfaceDescriptor; schemas: Schema; digest: string }
export interface InterfaceIdentity { version: string; digest: string }
export const ERROR_CODES = {
  PARSE_ERROR: -32700, INVALID_REQUEST: -32600, METHOD_NOT_FOUND: -32601,
  INVALID_ARGUMENT: -32602, INTERNAL_ERROR: -32603, NOT_READY: -32001,
  PROTOCOL_MISMATCH: -32002, CONTRACT_MISMATCH: -32003, CANCELLED: -32004,
  DEADLINE_EXCEEDED: -32005, OVERLOADED: -32006, PLUGIN_EXITED: -32007,
  CONNECTION_CLOSED: -32008, REENTRANT_CALL: -32009, INVALID_RESULT: -32010,
  STATE_INCOMPATIBLE: -32011, APPLICATION_ERROR: -32012,
} as const;
export type ErrorCode = keyof typeof ERROR_CODES;
export class PluginError extends Error {
  readonly code: ErrorCode;
  readonly details: JsonValue | undefined;
  readonly domainCode: string | undefined;
  readonly data: JsonValue | undefined;
  constructor(code: ErrorCode, message: string, details?: JsonValue, domainCode?: string, data?: JsonValue) {
    super(message); this.name = 'PluginError'; this.code = code; this.details = details; this.domainCode = domainCode; this.data = data;
  }
  static domain(domainCode: string, data: JsonValue, message = domainCode): PluginError {
    return new PluginError('APPLICATION_ERROR', message, undefined, domainCode, data);
  }
  toWire(): JsonObject {
    const data: JsonObject = { code: this.code };
    if (this.details !== undefined) data.details = this.details;
    if (this.domainCode !== undefined) data.domainCode = this.domainCode;
    if (this.data !== undefined) data.data = this.data;
    return { code: ERROR_CODES[this.code], message: truncateMessage(this.message), data };
  }
}
// JSON Schema lengths count Unicode scalars; never split a surrogate pair.
function truncateMessage(message: string): string {
  let end = 0, count = 0;
  for (const character of message) { if (count++ === 2048) break; end += character.length; }
  return message.slice(0, end);
}
export function asPluginError(error: unknown): PluginError {
  return error instanceof PluginError ? error : new PluginError('INTERNAL_ERROR', 'Handler failed');
}
export const DEFAULT_LIMITS = Object.freeze({
  maxFrameBytes: 8 * 1024 * 1024, maxDepth: 64, maxPendingCalls: 256,
  maxQueuedBytes: 16 * 1024 * 1024, maxCallChainLength: 16,
  callTimeoutMs: 30000, startupTimeoutMs: 10000, shutdownTimeoutMs: 5000,
  maxLogBytes: 1024 * 1024, maxEventQueue: 256,
});
export type Limits = { -readonly [K in keyof typeof DEFAULT_LIMITS]: number };
export function effectiveLimits(values: Partial<Limits> = {}): Limits {
  const result: Limits = { ...DEFAULT_LIMITS };
  for (const key of Object.keys(result) as (keyof Limits)[]) {
    const value = values[key];
    if (value !== undefined) {
      if (!Number.isSafeInteger(value) || value < 1 || value > 0x7fffffff) throw new PluginError('INVALID_ARGUMENT', `Invalid limit ${key}`);
      result[key] = value;
    }
  }
  return result;
}
export function negotiateLimits(local: Limits, remote: Partial<Limits>): Limits {
  const limits = effectiveLimits(remote);
  for (const key of Object.keys(local) as (keyof Limits)[]) limits[key] = Math.min(local[key], limits[key]);
  return limits;
}
export function object(value: unknown, label = 'value'): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new PluginError('INVALID_ARGUMENT', `${label} must be an object`);
  return value as Record<string, unknown>;
}
export function exactKeys(value: Record<string, unknown>, allowed: readonly string[], required: readonly string[] = [], label = 'value'): void {
  for (const key of Object.keys(value)) if (!allowed.includes(key)) throw new PluginError('INVALID_ARGUMENT', `${label}: unknown field ${key}`);
  for (const key of required) if (!Object.hasOwn(value, key)) throw new PluginError('INVALID_ARGUMENT', `${label}: missing ${key}`);
}
export function identifier(value: unknown): value is string { return typeof value === 'string' && value.length <= 128 && /^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*$/.test(value); }
export function version(value: unknown): value is string { return typeof value === 'string' && /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(value) && value.length <= 128; }
export function validString(value: string): boolean { return !/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?:^|[^\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(value); }
/** Copies only data descriptors. Never invokes toJSON/getters or mutates prototypes. */
export function wireValue(value: unknown, maxDepth = 64): JsonValue {
  const ancestors = new Set<object>();
  function copy(v: unknown, depth: number): JsonValue {
    if (depth > maxDepth) throw new PluginError('INVALID_ARGUMENT', 'Maximum JSON nesting exceeded');
    if (v === null || typeof v === 'boolean') return v;
    if (typeof v === 'string') { if (!validString(v)) throw new PluginError('INVALID_ARGUMENT', 'Unpaired Unicode surrogate'); return v; }
    if (typeof v === 'number') { if (!Number.isFinite(v)) throw new PluginError('INVALID_ARGUMENT', 'Non-finite number'); return Object.is(v, -0) ? 0 : v; }
    if (typeof v !== 'object') throw new PluginError('INVALID_ARGUMENT', `Not a wire value: ${typeof v}`);
    if (ancestors.has(v)) throw new PluginError('INVALID_ARGUMENT', 'Cyclic wire data');
    if (!Array.isArray(v) && Object.getPrototypeOf(v) !== Object.prototype && Object.getPrototypeOf(v) !== null) throw new PluginError('INVALID_ARGUMENT', 'Only plain data objects are wire values');
    if (Object.getOwnPropertySymbols(v).length) throw new PluginError('INVALID_ARGUMENT', 'Symbol properties are not wire values');
    ancestors.add(v);
    let out: JsonValue;
    if (Array.isArray(v)) {
      out = [];
      for (let i = 0; i < v.length; i++) {
        const d = Object.getOwnPropertyDescriptor(v, String(i));
        if (!d || !('value' in d)) throw new PluginError('INVALID_ARGUMENT', 'Sparse arrays and accessors are not wire data');
        out.push(copy(d.value, depth + 1));
      }
      if (Object.keys(v).some(k => !/^(0|[1-9]\d*)$/.test(k) || Number(k) >= v.length)) throw new PluginError('INVALID_ARGUMENT', 'Array properties are not wire data');
    } else {
      out = Object.create(null) as JsonObject;
      for (const k of Object.getOwnPropertyNames(v)) {
        if (!validString(k)) throw new PluginError('INVALID_ARGUMENT', 'Invalid object key');
        const d = Object.getOwnPropertyDescriptor(v, k)!;
        if (!('value' in d) || !d.enumerable) throw new PluginError('INVALID_ARGUMENT', 'Accessors and hidden fields are not wire data');
        out[k] = copy(d.value, depth + 1);
      }
    }
    ancestors.delete(v); return out;
  }
  return copy(value, 0);
}
export function decimal(value: bigint, unsigned = false): string {
  const min = unsigned ? 0n : -(1n << 63n), max = unsigned ? (1n << 64n) - 1n : (1n << 63n) - 1n;
  if (value < min || value > max) throw new PluginError('INVALID_ARGUMENT', 'Decimal outside 64-bit range');
  return value.toString();
}
export function bytes(value: Uint8Array): string { return Buffer.from(value).toString('base64'); }
export function decodeBytes(value: string): Uint8Array { checkWireType(value, 'bytes-base64'); return Buffer.from(value, 'base64'); }
function checkWireType(value: string, annotation: unknown): void {
  if (annotation === 'i64-decimal' || annotation === 'u64-decimal') {
    if (!/^(0|-?[1-9][0-9]*)$/.test(value) || value.length > 20) throw new Error('Noncanonical decimal');
    decimal(BigInt(value), annotation === 'u64-decimal');
  } else if (annotation === 'bytes-base64') {
    if (Buffer.from(value, 'base64').toString('base64') !== value) throw new Error('Noncanonical base64');
  } else if (annotation === 'utc-datetime') {
    if (!/^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3}Z$/.test(value) || value.startsWith('0000') || !Number.isFinite(Date.parse(value)) || new Date(value).toISOString() !== value) throw new Error('Invalid UTC Gregorian datetime');
  } else if (annotation !== undefined) throw new Error('Unknown wire annotation');
}
export function validate(contract: Contract, ref: string, value: unknown, code: ErrorCode = 'INVALID_ARGUMENT'): JsonValue {
  let data: JsonValue;
  try { data = wireValue(value); } catch (error) { throw new PluginError(code, error instanceof Error ? error.message : 'Invalid wire data'); }
  try { validateSchema(contract.schemas, { $ref: ref }, data, '#', 0); }
  catch (error) { throw new PluginError(code, `${contract.descriptor.id} ${ref}: ${error instanceof Error ? error.message : String(error)}`); }
  return data;
}
export function validateSchema(root: Schema, schema: Schema, value: JsonValue, path = '#', depth = 0): void {
  if (depth > 128) throw new Error(`${path}: schema reference/depth limit`);
  const fail = (message: string): never => { throw new Error(`${path}: ${message}`); };
  if (schema.$ref !== undefined) {
    const ref = schema.$ref;
    if (typeof ref !== 'string' || !ref.startsWith('#/')) return fail('only local references supported');
    let target: unknown = root;
    for (const p of ref.slice(2).split('/')) { const key = p.replace(/~1/g, '/').replace(/~0/g, '~'); if (!target || typeof target !== 'object' || !Object.hasOwn(target, key)) return fail(`unresolved ${ref}`); target = (target as Schema)[key]; }
    return validateSchema(root, target as Schema, value, path, depth + 1);
  }
  if (schema.oneOf) {
    const variants = schema.oneOf as Schema[];
    let matches = 0;
    for (const variant of variants) { try { validateSchema(root, variant, value, path, depth + 1); matches++; } catch { /* try next declared variant */ } }
    if (matches !== 1) fail('must match exactly one union variant'); return;
  }
  if (Array.isArray(schema.type)) {
    const types = schema.type as string[];
    if (value === null && types.includes('null')) return;
    return validateSchema(root, { ...schema, type: types.find(t => t !== 'null') }, value, path, depth + 1);
  }
  if (Object.hasOwn(schema, 'const') && JSON.stringify(value) !== JSON.stringify(schema.const)) fail('literal does not match');
  if (schema.enum && !(schema.enum as unknown[]).includes(value)) fail('not an allowed enum value');
  if (schema.type === 'object') {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) return fail('expected object');
    const props = (schema.properties ?? {}) as Record<string, Schema>;
    for (const key of (schema.required ?? []) as string[]) if (!Object.hasOwn(value, key)) fail(`missing ${key}`);
    for (const key of Object.keys(value)) { if (!Object.hasOwn(props, key)) fail(`unknown field ${key}`); validateSchema(root, props[key]!, value[key]!, `${path}/${key}`, depth + 1); }
  } else if (schema.type === 'array') {
    if (!Array.isArray(value)) return fail('expected array');
    if (typeof schema.minItems === 'number' && value.length < schema.minItems || typeof schema.maxItems === 'number' && value.length > schema.maxItems) fail('array length out of bounds');
    value.forEach((v, i) => validateSchema(root, schema.items as Schema, v, `${path}/${i}`, depth + 1));
  } else if (schema.type === 'string') {
    if (typeof value !== 'string') return fail('expected string');
    let len = 0;
    // Count scalars without allocating an array proportional to frame size.
    if (typeof schema.minLength === 'number' || typeof schema.maxLength === 'number') {
      for (const _character of value) {
        len++;
        if (typeof schema.maxLength === 'number' && len > schema.maxLength) break;
      }
    }
    if (typeof schema.minLength === 'number' && len < schema.minLength || typeof schema.maxLength === 'number' && len > schema.maxLength) fail('string length out of bounds');
    checkWireType(value, schema['x-wire-type']);
  } else if (schema.type === 'integer' || schema.type === 'number') {
    if (typeof value !== 'number' || !Number.isFinite(value)) return fail('expected finite number');
    if (schema.type === 'integer' && !Number.isSafeInteger(value)) fail('expected safe integer');
    if (typeof schema.minimum === 'number' && value < schema.minimum || typeof schema.maximum === 'number' && value > schema.maximum) fail('number out of bounds');
  } else if (schema.type === 'boolean') { if (typeof value !== 'boolean') fail('expected boolean'); }
  else if (schema.type === 'null') { if (value !== null) fail('expected null'); }
  else if (!Object.hasOwn(schema, 'const') && !schema.enum) fail('unsupported schema');
}
export function identities(contracts: Iterable<Contract>): Record<string, InterfaceIdentity> {
  const out: Record<string, InterfaceIdentity> = Object.create(null) as Record<string, InterfaceIdentity>;
  for (const c of contracts) out[c.descriptor.id] = { version: c.descriptor.version, digest: c.digest };
  return out;
}
export function matchIdentities(expected: Record<string, InterfaceIdentity>, actual: unknown): void {
  const values = object(actual, 'interfaces');
  if (Object.keys(values).length !== Object.keys(expected).length) throw new PluginError('CONTRACT_MISMATCH', 'Interface set differs');
  for (const [id, identity] of Object.entries(expected)) {
    const found = object(values[id], `interface ${id}`); exactKeys(found, ['version', 'digest'], ['version', 'digest']);
    if (found.version !== identity.version || found.digest !== identity.digest) throw new PluginError('CONTRACT_MISMATCH', `Interface version/digest mismatch: ${id}`);
  }
}
export function encodeFrame(value: unknown, maxBytes = DEFAULT_LIMITS.maxFrameBytes, maxDepth: number = DEFAULT_LIMITS.maxDepth): Buffer {
  const payload = Buffer.from(JSON.stringify(wireValue(value, maxDepth)), 'utf8');
  if (payload.length === 0 || payload.length > maxBytes) throw new PluginError('OVERLOADED', `Frame exceeds ${maxBytes} bytes`);
  const result = Buffer.allocUnsafe(payload.length + 4); result.writeUInt32BE(payload.length); payload.copy(result, 4); return result;
}
/** Bounded incremental codec: allocates payload only after validating its header. */
export class FrameDecoder {
  private header = Buffer.alloc(4); private headerUsed = 0; private payload: Buffer | undefined; private used = 0;
  maxBytes: number; maxDepth: number;
  constructor(maxBytes = DEFAULT_LIMITS.maxFrameBytes, maxDepth: number = DEFAULT_LIMITS.maxDepth) { this.maxBytes = maxBytes; this.maxDepth = maxDepth; }
  push(chunk: Uint8Array, onFrame?: (value: JsonValue) => boolean): JsonValue[] {
    const frames: JsonValue[] = []; let offset = 0; let decodedBytes = 0; let decodedFrames = 0;
    while (offset < chunk.length) {
      if (!this.payload) {
        const count = Math.min(4 - this.headerUsed, chunk.length - offset); this.header.set(chunk.subarray(offset, offset + count), this.headerUsed); this.headerUsed += count; offset += count;
        if (this.headerUsed !== 4) continue;
        const size = this.header.readUInt32BE(); this.headerUsed = 0;
        if (size === 0 || size > this.maxBytes) throw new PluginError('INVALID_REQUEST', `Invalid frame length ${size}`);
        this.payload = Buffer.allocUnsafe(size); this.used = 0;
      }
      const count = Math.min(this.payload.length - this.used, chunk.length - offset); this.payload.set(chunk.subarray(offset, offset + count), this.used); offset += count; this.used += count;
      if (this.used === this.payload.length) {
        const payload = this.payload; this.payload = undefined; this.used = 0;
        decodedBytes += payload.length; if (++decodedFrames > 256 || decodedBytes > 16 * 1024 * 1024) throw new PluginError('OVERLOADED', 'Coalesced frame admission limit exceeded');
        let parsed: unknown;
        try { const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(payload); parsed = JSON.parse(text); }
        catch { throw new PluginError('PARSE_ERROR', 'Invalid UTF-8 or JSON'); }
        const value = wireValue(parsed, this.maxDepth);
        // Dispatch before decoding the next frame: hello may tighten limits.
        if (onFrame) { if (!onFrame(value)) break; }
        else frames.push(value);
      }
    }
    return frames;
  }
  finish(): void { if (this.headerUsed || this.payload) throw new PluginError('INVALID_REQUEST', 'Truncated frame'); }
}
export interface WireContext { timeoutMs: number; callChain: string[] }
export interface CallOptions { timeoutMs?: number; signal?: AbortSignal; context?: { callChain: readonly string[]; remainingMs?: () => number; signal?: AbortSignal } }
export interface RequestContext {
  id: string; signal: AbortSignal; callChain: string[]; remainingMs(): number; throwIfCancelled(): void;
}
interface Pending { resolve(v: JsonValue): void; reject(e: unknown): void; timer: ReturnType<typeof setTimeout>; cleanup(): void; business: boolean; acceptResult?: (value: JsonValue) => void }
export interface RpcPeerOptions { role: 'h' | 'p'; sessionId: string; limits?: Partial<Limits>; bootstrap?: boolean }
export type RpcHandler = (params: JsonValue, context: RequestContext) => JsonValue | Promise<JsonValue>;
export class RpcPeer extends EventEmitter {
  readonly socket: Socket; readonly endpointId: string; readonly sessionId: string;
  limits: Limits; readonly decoder: FrameDecoder;
  private counter = 0n; private pending = new Map<string, Pending>();
  private active = new Map<string, AbortController>(); private handlers = new Map<string, RpcHandler>();
  private occupied = false; private closed = false; private queuedBytes = 0;
  private writes: { frame: Buffer; priority: boolean; event: boolean }[] = []; private writing = false; private eventFence: string | undefined;
  private idleWaiters = new Set<() => void>(); private readonly prefix: string;
  diagnostics = { unknownReplies: 0, unknownNotifications: 0 };
  constructor(socket: Socket, options: RpcPeerOptions) {
    super(); this.socket = socket; this.sessionId = options.sessionId; this.endpointId = `${options.role}:${options.sessionId}`; this.prefix = `${this.endpointId}:`;
    this.limits = effectiveLimits(options.limits); this.decoder = new FrameDecoder(options.bootstrap ? 65536 : this.limits.maxFrameBytes, this.limits.maxDepth);
    socket.setNoDelay(true);
    socket.on('data', chunk => { try { this.decoder.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk, value => { this.receive(value); return !this.closed; }); } catch (error) { this.close(asPluginError(error)); } });
    socket.on('end', () => { try { this.decoder.finish(); } catch (error) { this.close(asPluginError(error)); } });
    socket.on('error', () => this.close(new PluginError('CONNECTION_CLOSED', 'Transport error')));
    socket.on('close', () => this.close());
  }
  get isClosed(): boolean { return this.closed; }
  get busy(): boolean { return this.occupied; }
  negotiate(remote: Partial<Limits>): void { this.limits = negotiateLimits(this.limits, remote); this.decoder.maxBytes = this.limits.maxFrameBytes; this.decoder.maxDepth = this.limits.maxDepth; }
  onRequest(method: string, handler: RpcHandler): this { if (this.handlers.has(method)) throw new PluginError('INVALID_ARGUMENT', `Duplicate handler ${method}`); this.handlers.set(method, handler); return this; }
  // Keep events bounded in the normal outbound queue until this success reply is queued.
  holdEventsUntilReply(method: string): void { this.eventFence = method; }
  // acceptResult runs in wire order, before any following frame or promise continuation.
  request(method: string, params: unknown, options: CallOptions = {}, acceptResult?: (value: JsonValue) => void): Promise<JsonValue> {
    if (this.closed) return Promise.reject(new PluginError('CONNECTION_CLOSED', 'Peer is closed'));
    const business = method === 'system.invoke';
    if (this.pending.size >= this.limits.maxPendingCalls + (business ? 0 : 16)) return Promise.reject(new PluginError('OVERLOADED', 'Pending request capacity reached'));
    const signal = options.signal ?? options.context?.signal;
    if (signal?.aborted) return Promise.reject(new PluginError('CANCELLED', 'Call cancelled before sending'));
    if (options.context?.remainingMs && options.context.remainingMs() <= 0) return Promise.reject(new PluginError('DEADLINE_EXCEEDED', 'Parent deadline has already expired'));
    const supplied = options.timeoutMs ?? this.limits.callTimeoutMs;
    if (!Number.isFinite(supplied) || supplied <= 0) return Promise.reject(new PluginError('INVALID_ARGUMENT', 'timeoutMs must be positive'));
    const timeoutMs = Math.max(1, Math.min(supplied, options.context?.remainingMs?.() ?? supplied, this.limits.callTimeoutMs));
    const id = `${this.prefix}${++this.counter}`;
    return new Promise((resolve, reject) => {
      const settle = (code: 'CANCELLED' | 'DEADLINE_EXCEEDED') => {
        const entry = this.pending.get(id); if (!entry) return;
        this.pending.delete(id); entry.cleanup(); reject(new PluginError(code, code === 'CANCELLED' ? 'Call cancelled; remote effects may already have occurred' : 'Deadline exceeded; remote outcome is unknown'));
        try { this.notify('system.cancel', { id }, true); } catch { /* best effort */ }
      };
      const cancel = () => settle('CANCELLED');
      const timer = setTimeout(() => settle('DEADLINE_EXCEEDED'), timeoutMs);
      const cleanup = () => { clearTimeout(timer); signal?.removeEventListener('abort', cancel); };
      this.pending.set(id, { resolve, reject, timer, cleanup, business, ...(acceptResult ? { acceptResult } : {}) }); signal?.addEventListener('abort', cancel, { once: true });
      try { this.send({ jsonrpc: '2.0', id, method, params: wireValue(params) }, !business); }
      catch (error) { this.pending.delete(id); cleanup(); reject(error); }
    });
  }
  notify(method: string, params: unknown, priority = false): void { this.send({ jsonrpc: '2.0', method, params: wireValue(params) }, priority); }
  private send(message: unknown, priority: boolean): void {
    if (this.closed) throw new PluginError('CONNECTION_CLOSED', 'Peer is closed');
    const frame = encodeFrame(message, this.decoder.maxBytes, this.limits.maxDepth);
    const cap = this.limits.maxQueuedBytes + (priority ? 1024 * 1024 : 0);
    if (this.queuedBytes + frame.length > cap) throw new PluginError('OVERLOADED', 'Outbound byte queue is full');
    this.queuedBytes += frame.length;
    const event = (message as JsonObject).method === 'system.event';
    if (priority) { const firstNormal = this.writes.findIndex(w => !w.priority); if (firstNormal < 0) this.writes.push({ frame, priority, event }); else this.writes.splice(firstNormal, 0, { frame, priority, event }); }
    else this.writes.push({ frame, priority, event });
    this.flush();
  }
  private flush(): void {
    if (this.writing || this.closed) return; const index = this.writes.findIndex(write => !write.event || this.eventFence === undefined); if (index < 0) return; const next = this.writes.splice(index, 1)[0]!; this.writing = true;
    this.socket.write(next.frame, error => { this.queuedBytes -= next.frame.length; this.writing = false; if (error) this.close(new PluginError('CONNECTION_CLOSED', 'Write failed')); else this.flush(); });
  }
  private reply(id: string, result: JsonValue | undefined, error?: PluginError, method?: string): void {
    try { this.send(error ? { jsonrpc: '2.0', id, error: error.toWire() } : { jsonrpc: '2.0', id, result: result ?? null }, true); if (!error && method === this.eventFence) { this.eventFence = undefined; this.flush(); } }
    catch (failure) { this.close(asPluginError(failure)); }
  }
  private receive(value: JsonValue): void {
    const msg = object(value, 'RPC envelope');
    if (msg.jsonrpc !== '2.0') throw new PluginError('INVALID_REQUEST', 'Expected JSON-RPC 2.0');
    if (Object.hasOwn(msg, 'method')) {
      exactKeys(msg, ['jsonrpc', 'id', 'method', 'params'], ['jsonrpc', 'method']);
      if (typeof msg.method !== 'string' || msg.method.length > 128 || !Object.hasOwn(msg, 'params')) throw new PluginError('INVALID_REQUEST', 'Invalid request method/params');
      if (!Object.hasOwn(msg, 'id')) {
        if (msg.method === 'system.invoke') throw new PluginError('INVALID_REQUEST', 'Business invocation requires an ID');
        if (msg.method === 'system.cancel') { const p = object(msg.params); if (typeof p.id !== 'string') throw new PluginError('INVALID_REQUEST', 'Invalid cancellation id'); this.active.get(p.id)?.abort(); }
        else if (msg.method === 'system.event') this.emit('event', msg.params);
        else this.diagnostics.unknownNotifications = Math.min(Number.MAX_SAFE_INTEGER, this.diagnostics.unknownNotifications + 1);
        return;
      }
      const id = msg.id;
      if (typeof id !== 'string' || !id.startsWith(`${this.endpointId.startsWith('h:') ? 'p' : 'h'}:${this.sessionId}:`) || !/^[hp]:[^:]{1,128}:[1-9][0-9]{0,30}$/.test(id)) throw new PluginError('INVALID_REQUEST', 'Invalid namespaced request ID');
      if (this.active.has(id)) throw new PluginError('INVALID_REQUEST', 'Duplicate active request ID');
      const handler = this.handlers.get(msg.method);
      if (!handler) { this.reply(id, undefined, new PluginError('METHOD_NOT_FOUND', `Unknown method ${msg.method}`)); return; }
      const business = msg.method === 'system.invoke';
      let chain: string[] = []; let timeoutMs = this.limits.callTimeoutMs;
      if (business) {
        const params = object(msg.params); const ctx = object(params.context, 'call context');
        if (!Array.isArray(ctx.callChain) || ctx.callChain.some(x => typeof x !== 'string' || x.length > 160) || ctx.callChain.length >= this.limits.maxCallChainLength || typeof ctx.timeoutMs !== 'number' || !Number.isSafeInteger(ctx.timeoutMs) || ctx.timeoutMs <= 0) { this.reply(id, undefined, new PluginError('INVALID_ARGUMENT', 'Invalid call context')); return; }
        chain = ctx.callChain as string[]; timeoutMs = Math.min(ctx.timeoutMs, this.limits.callTimeoutMs);
        if (chain.includes(this.endpointId) || this.occupied) { this.reply(id, undefined, new PluginError('REENTRANT_CALL', 'Endpoint has an active business/lifecycle handler')); return; }
        this.occupied = true;
      }
      if (this.active.size >= this.limits.maxPendingCalls + 16) { if (business) this.release(); this.reply(id, undefined, new PluginError('OVERLOADED', 'Inbound request capacity reached')); return; }
      const controller = new AbortController(); this.active.set(id, controller); const deadline = performance.now() + timeoutMs;
      const timer = setTimeout(() => controller.abort(), timeoutMs);
      const context: RequestContext = { id, signal: controller.signal, callChain: [...chain, this.endpointId], remainingMs: () => Math.max(0, deadline - performance.now()), throwIfCancelled() { if (controller.signal.aborted) throw new PluginError('CANCELLED', 'Handler acknowledged cancellation'); } };
      void Promise.resolve().then(() => handler(msg.params as JsonValue, context)).then(result => {
        let data: JsonValue;
        try { data = wireValue(result, this.limits.maxDepth); }
        catch { throw new PluginError('INVALID_RESULT', 'Handler returned invalid wire data'); }
        this.reply(id, data, undefined, msg.method as string);
      }).catch(error => this.reply(id, undefined, asPluginError(error))).finally(() => { clearTimeout(timer); this.active.delete(id); if (business) this.release(); });
    } else {
      exactKeys(msg, ['jsonrpc', 'id', 'result', 'error'], ['jsonrpc', 'id']);
      if (typeof msg.id !== 'string' || Object.hasOwn(msg, 'result') === Object.hasOwn(msg, 'error')) throw new PluginError('INVALID_REQUEST', 'Malformed RPC response');
      const pending = this.pending.get(msg.id);
      if (!pending) { this.diagnostics.unknownReplies = Math.min(Number.MAX_SAFE_INTEGER, this.diagnostics.unknownReplies + 1); return; }
      let failure: PluginError | undefined;
      if (Object.hasOwn(msg, 'error')) {
        const e = object(msg.error); exactKeys(e, ['code', 'message', 'data'], ['code', 'message', 'data']); const data = object(e.data);
        exactKeys(data, ['code', 'details', 'domainCode', 'data'], ['code']);
        if (Object.hasOwn(data, 'domainCode') && (typeof data.domainCode !== 'string' || !data.domainCode || data.code !== 'APPLICATION_ERROR') || Object.hasOwn(data, 'data') && data.code !== 'APPLICATION_ERROR' || data.code === 'APPLICATION_ERROR' && (!Object.hasOwn(data, 'domainCode') || !Object.hasOwn(data, 'data'))) throw new PluginError('INVALID_REQUEST', 'Malformed application error');
        if (typeof data.code !== 'string' || !Object.hasOwn(ERROR_CODES, data.code) || ERROR_CODES[data.code as ErrorCode] !== e.code || typeof e.message !== 'string' || truncateMessage(e.message).length !== e.message.length) throw new PluginError('INVALID_REQUEST', 'Malformed RPC error');
        failure = new PluginError(data.code as ErrorCode, e.message, data.details as JsonValue | undefined, data.domainCode as string | undefined, data.data as JsonValue | undefined);
      }
      this.pending.delete(msg.id); pending.cleanup();
      if (failure) pending.reject(failure);
      else { try { pending.acceptResult?.(msg.result as JsonValue); pending.resolve(msg.result as JsonValue); } catch (error) { pending.reject(asPluginError(error)); } }
    }
  }
  private release(): void { this.occupied = false; for (const resolve of this.idleWaiters) resolve(); this.idleWaiters.clear(); }
  async whenIdle(timeoutMs: number): Promise<void> {
    if (!this.occupied) return;
    await new Promise<void>((resolve, reject) => { const done = () => { clearTimeout(timer); this.idleWaiters.delete(done); resolve(); }; const timer = setTimeout(() => { this.idleWaiters.delete(done); reject(new PluginError('DEADLINE_EXCEEDED', 'Quiescence deadline exceeded')); }, timeoutMs); this.idleWaiters.add(done); });
  }
  cancelActive(exceptId?: string): void { for (const [id, controller] of this.active) if (id !== exceptId) controller.abort(); }
  close(error = new PluginError('CONNECTION_CLOSED', 'Connection closed')): void {
    if (this.closed) return; this.closed = true; this.cancelActive(); this.socket.destroy();
    for (const pending of this.pending.values()) { pending.cleanup(); pending.reject(error); } this.pending.clear(); for (const write of this.writes) this.queuedBytes -= write.frame.length; this.writes = []; this.emit('closed', error);
  }
}

export const validateRef = validate;
export function canonical(value: JsonValue): string {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  if (value && typeof value === 'object') return '{' + Object.keys(value).sort().map(key => JSON.stringify(key) + ':' + canonical(value[key]!)).join(',') + '}';
  return JSON.stringify(value);
}
/** Validates bundled normalized schema metadata even when not produced by our generator. */
export function assertContract(value: unknown): asserts value is Contract {
  const c = object(wireValue(value)); exactKeys(c, ['descriptor', 'schemas', 'digest'], ['descriptor', 'schemas', 'digest']);
  const d = object(c.descriptor); exactKeys(d, ['descriptorVersion', 'id', 'version', 'methods', 'errors', 'events', 'state', 'title', 'description'], ['descriptorVersion', 'id', 'version', 'methods', 'errors', 'events']);
  if (d.descriptorVersion !== 1 || !identifier(d.id) || !version(d.version)) throw new PluginError('CONTRACT_MISMATCH', 'Invalid contract identity');
  const root = object(c.schemas); exactKeys(root, ['$defs'], ['$defs']); const defs = object(root.$defs); if (Object.keys(defs).length > 1024) throw new PluginError('CONTRACT_MISMATCH', 'Too many schema definitions');
  const checked = new Set<string>(); const visiting = new Set<string>();
  const fail = (at: string, detail: string): never => { throw new PluginError('CONTRACT_MISMATCH', `${d.id} ${at}: ${detail}`); };
  function ref(value: unknown, at: string): void {
    if (typeof value !== 'string' || !value.startsWith('#/$defs/') || /~(?![01])/.test(value)) return fail(at, 'Expected bundled local $defs reference');
    const name = value.slice(8).replace(/~1/g, '/').replace(/~0/g, '~');
    if (!Object.hasOwn(defs, name)) return fail(at, 'Unknown reference');
    if (visiting.has(name)) return fail(at, 'Recursive schema reference');
    if (checked.has(name)) return;
    if (visiting.size >= 32) return fail(at, 'Reference depth limit');
    visiting.add(name); schema(defs[name], value, 0); visiting.delete(name); checked.add(name);
  }
  function schema(raw: unknown, at: string, depth: number): void {
    if (depth > 64) return fail(at, 'Schema nesting limit'); const s = object(raw, at);
    for (const key of ['title', 'description']) if (s[key] !== undefined && typeof s[key] !== 'string') fail(at, 'Invalid metadata');
    if (s.$ref !== undefined) { exactKeys(s, ['$ref', 'title', 'description']); ref(s.$ref, at); return; }
    if (s.oneOf !== undefined) {
      exactKeys(s, ['oneOf', 'title', 'description']); if (!Array.isArray(s.oneOf) || s.oneOf.length < 2 || s.oneOf.length > 32) return fail(at, 'Tagged union requires 2–32 variants');
      const variants = s.oneOf.map((v, i) => { schema(v, `${at}/oneOf/${i}`, depth + 1); return object(v); });
      const first = object(variants[0]!.properties);
      const tag = Object.keys(first).find(key => variants.every(v => v.type === 'object' && Array.isArray(v.required) && v.required.includes(key) && typeof object(object(v.properties)[key]).const === 'string') && new Set(variants.map(v => object(object(v.properties)[key]).const)).size === variants.length);
      if (!tag) fail(at, 'Union requires a shared required unique string discriminator'); return;
    }
    let type = s.type;
    if (Array.isArray(type)) { if (s.enum !== undefined || Object.hasOwn(s, 'const')) return fail(at, 'Nullable enum/literal combination is outside the v1 profile'); if (type.length !== 2 || !type.includes('null') || type[0] === type[1]) return fail(at, 'Nullable schema syntax is type:[nonNullType,"null"]'); type = type.find(t => t !== 'null'); }
    if (typeof type !== 'string' || !['object', 'array', 'string', 'integer', 'number', 'boolean', 'null'].includes(type)) return fail(at, 'Unknown/missing schema type');
    const keys: Record<string, string[]> = { object: ['properties', 'required', 'additionalProperties'], array: ['items', 'minItems', 'maxItems'], string: ['minLength', 'maxLength', 'enum', 'const', 'x-wire-type'], integer: ['minimum', 'maximum', 'const'], number: ['minimum', 'maximum', 'const'], boolean: ['const'], null: ['const'] };
    exactKeys(s, ['type', 'title', 'description', ...keys[type]!]);
    if (type === 'object') {
      const props = object(s.properties); if (s.additionalProperties !== false || !Array.isArray(s.required) || new Set(s.required).size !== s.required.length || s.required.some(k => typeof k !== 'string' || !Object.hasOwn(props, k))) return fail(at, 'Objects require declared fields and additionalProperties:false');
      for (const [key, property] of Object.entries(props)) schema(property, `${at}/properties/${key}`, depth + 1);
    }
    if (type === 'array') schema(s.items, at + '/items', depth + 1);
    for (const [min, max] of [['minLength', 'maxLength'], ['minItems', 'maxItems'], ['minimum', 'maximum']] as const) {
      for (const k of [min, max]) if (s[k] !== undefined && (typeof s[k] !== 'number' || !Number.isFinite(s[k]) || (k !== 'minimum' && k !== 'maximum' && (!Number.isSafeInteger(s[k]) || (s[k] as number) < 0)))) fail(at, 'Invalid bound');
      if (s[min] !== undefined && s[max] !== undefined && (s[min] as number) > (s[max] as number)) fail(at, 'Conflicting bounds');
    }
    if (s.enum !== undefined && (!Array.isArray(s.enum) || !s.enum.length || s.enum.some(x => typeof x !== 'string') || new Set(s.enum).size !== s.enum.length)) fail(at, 'Enum requires distinct strings');
    if (Object.hasOwn(s, 'const') && !(type === 'null' ? s.const === null : type === 'integer' ? typeof s.const === 'number' && Number.isSafeInteger(s.const) : typeof s.const === type)) fail(at, 'Literal type mismatch');
    if (s.enum && s.const !== undefined && !(s.enum as unknown[]).includes(s.const)) fail(at, 'Conflicting enum/literal');
    if (s['x-wire-type'] !== undefined && !['i64-decimal', 'u64-decimal', 'bytes-base64', 'utc-datetime'].includes(s['x-wire-type'] as string)) fail(at, 'Unknown wire annotation');
    for (const literal of [...((s.enum ?? []) as JsonValue[]), ...(s.const !== undefined ? [s.const as JsonValue] : [])]) try { validateSchema(root, s, literal); } catch { fail(at, 'Literal conflicts with schema'); }
  }
  for (const key of Object.keys(defs)) ref('#/$defs/' + key.replace(/~/g, '~0').replace(/\//g, '~1'), '#/$defs');
  const errors = object(d.errors);
  for (const [category, members] of [['methods', object(d.methods)], ['errors', errors], ['events', object(d.events)]] as const) {
    if (Object.keys(members).length > 256) fail(category, 'Too many members');
    for (const [name, raw] of Object.entries(members)) {
      if (!name || name.length > 200) fail(category, 'Invalid member name'); const member = object(raw);
      const refs = category === 'methods' ? ['input', 'output'] : category === 'errors' ? ['data'] : ['payload'];
      exactKeys(member, [...refs, 'description', ...(category === 'methods' ? ['errors', 'idempotent'] : [])], refs);
      for (const field of refs) ref(member[field], `${category}/${name}/${field}`);
      if (category === 'methods') { if (!Array.isArray(member.errors) || new Set(member.errors).size !== member.errors.length || member.errors.some(e => typeof e !== 'string' || !Object.hasOwn(errors, e))) fail(category, 'Invalid declared errors'); if (member.idempotent !== undefined && typeof member.idempotent !== 'boolean') fail(category, 'Invalid idempotence'); }
    }
  }
  if (d.state !== undefined) { const state = object(d.state); exactKeys(state, ['contract', 'version', 'schema'], ['contract', 'version', 'schema']); if (!identifier(state.contract) || !Number.isSafeInteger(state.version) || (state.version as number) < 1) fail('state', 'Invalid state identity'); ref(state.schema, 'state/schema'); }
  const digest = createHash('sha256').update(canonical(wireValue({ descriptor: c.descriptor, schemas: c.schemas })) + '\n').digest('hex');
  if (c.digest !== digest) fail('digest', 'Contract metadata digest mismatch');
}
