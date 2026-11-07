import { connect } from 'node:net';
import { Contract, JsonValue, JsonObject, RpcPeer, PluginError, CallOptions, RequestContext, Limits, effectiveLimits, validate, wireValue, object, exactKeys, identities, matchIdentities, asPluginError, assertContract, DEFAULT_LIMITS } from '@napi-vm/plugin-protocol';
export { PluginError } from '@napi-vm/plugin-protocol';
export type { Contract, CallOptions, JsonValue } from '@napi-vm/plugin-protocol';
export interface Invoker { invoke(contract: Contract, method: string, input: unknown, options?: CallOptions): Promise<JsonValue> }
export interface CallContext extends RequestContext, Invoker {
  client(contract: Contract): Record<string, (input: unknown, options?: CallOptions) => Promise<JsonValue>>;
  emit(contract: Contract, event: string, payload: unknown): Promise<void>;
  subscribe(contract: Contract, event: string, listener: (payload: JsonValue) => void | Promise<void>, onError?: (error: PluginError) => void): () => void;
  log(...values: unknown[]): void;
  spawnTask(task: (signal: AbortSignal) => Promise<void>): Promise<void>;
}
export type Handler = (input: JsonValue, context: CallContext) => unknown | Promise<unknown>;
export interface Snapshot { stateVersion: number; contract: string; data: JsonValue }
export interface InitializeInput { configuration: JsonValue; context: JsonValue; snapshot: Snapshot | null }
export interface LifecycleHooks {
  initialize?(input: InitializeInput, context: CallContext): void | Promise<void>;
  snapshot?(context: CallContext): unknown | Promise<unknown>;
  quiesce?(context: CallContext): void | Promise<void>;
  shutdown?(context: CallContext): void | Promise<void>;
}
export interface PluginDefinition { contract: Contract; handlers: Record<string, Handler>; hooks: LifecycleHooks }
export function definePlugin(contract: Contract, handlers: Record<string, Handler>, hooks: LifecycleHooks = {}): PluginDefinition {
  assertContract(contract);
  for (const method of Object.keys(contract.descriptor.methods)) if (!Object.hasOwn(handlers, method) || typeof handlers[method] !== 'function') throw new PluginError('INVALID_ARGUMENT', `Missing handler ${method}`);
  for (const method of Object.keys(handlers)) if (!Object.hasOwn(contract.descriptor.methods, method)) throw new PluginError('INVALID_ARGUMENT', `Undeclared handler ${method}`);
  return { contract, handlers, hooks };
}
export class PluginRegistry {
  readonly definitions = new Map<string, PluginDefinition>();
  add(definition: PluginDefinition): this { const id = definition.contract.descriptor.id; if (this.definitions.has(id)) throw new PluginError('INVALID_ARGUMENT', `Duplicate interface ${id}`); this.definitions.set(id, definition); return this; }
  get contracts(): Contract[] { return [...this.definitions.values()].map(x => x.contract); }
}
export function registry(plugin: PluginDefinition | PluginRegistry): PluginRegistry { return plugin instanceof PluginRegistry ? plugin : new PluginRegistry().add(plugin); }
export function createClient(contract: Contract, invoker: Invoker): Record<string, (input: unknown, options?: CallOptions) => Promise<JsonValue>> {
  const client: Record<string, (input: unknown, options?: CallOptions) => Promise<JsonValue>> = Object.create(null) as Record<string, (input: unknown, options?: CallOptions) => Promise<JsonValue>>;
  for (const method of Object.keys(contract.descriptor.methods)) client[method] = (input, options) => invoker.invoke(contract, method, input, options);
  return client;
}
export function validateDomainError(contract: Contract, method: string, error: unknown): PluginError {
  const failure = asPluginError(error);
  if (failure.code !== 'APPLICATION_ERROR') return failure;
  const domain = failure.domainCode;
  if (!domain || !contract.descriptor.methods[method]?.errors?.includes(domain) || !Object.hasOwn(contract.descriptor.errors, domain)) return new PluginError('INVALID_RESULT', 'Undeclared application error');
  try { validate(contract, contract.descriptor.errors[domain]!.data, failure.data, 'INVALID_RESULT'); return failure; }
  catch { return new PluginError('INVALID_RESULT', 'Invalid application error data'); }
}
export function validateSnapshot(contracts: readonly Contract[], value: unknown): Snapshot | null {
  const states = contracts.filter(c => c.descriptor.state);
  if (value === null) { if (states.length) throw new PluginError('STATE_INCOMPATIBLE', 'Stateful plugin must provide a snapshot'); return null; }
  const snap = object(value, 'snapshot'); exactKeys(snap, ['stateVersion', 'contract', 'data'], ['stateVersion', 'contract', 'data']);
  const contract = states.find(c => c.descriptor.state?.contract === snap.contract && c.descriptor.state?.version === snap.stateVersion);
  if (!contract) throw new PluginError('STATE_INCOMPATIBLE', 'Snapshot contract/version mismatch');
  return { stateVersion: snap.stateVersion as number, contract: snap.contract as string, data: validate(contract, contract.descriptor.state!.schema, snap.data, 'STATE_INCOMPATIBLE') };
}
export async function invokeRemote(peer: RpcPeer, contract: Contract, method: string, input: unknown, options: CallOptions = {}): Promise<JsonValue> {
  const descriptor = Object.hasOwn(contract.descriptor.methods, method) ? contract.descriptor.methods[method] : undefined;
  if (!descriptor) throw new PluginError('METHOD_NOT_FOUND', `Unknown business method ${method}`);
  const data = validate(contract, descriptor.input, input);
  const target = `${peer.endpointId.startsWith('h:') ? 'p' : 'h'}:${peer.sessionId}`;
  const chain = [...(options.context?.callChain ?? [])];
  if (chain.includes(target)) throw new PluginError('REENTRANT_CALL', 'Target is active in the call chain');
  const timeoutMs = Math.max(1, Math.floor(Math.min(options.timeoutMs ?? peer.limits.callTimeoutMs, options.context?.remainingMs?.() ?? peer.limits.callTimeoutMs, peer.limits.callTimeoutMs)));
  try {
    const result = await peer.request('system.invoke', { interface: contract.descriptor.id, version: contract.descriptor.version, method, input: data, context: { timeoutMs, callChain: chain } }, options);
    return validate(contract, descriptor.output, result, 'INVALID_RESULT');
  } catch (error) { throw validateDomainError(contract, method, error); }
}
class Tasks {
  controller = new AbortController(); pending = new Set<Promise<void>>(); failure: unknown; failed = false; draining = false;
  spawn(task: (signal: AbortSignal) => Promise<void>): Promise<void> {
    if (this.draining) throw new PluginError('NOT_READY', 'Background work is quiescing');
    const work = Promise.resolve().then(() => task(this.controller.signal)); this.pending.add(work);
    void work.catch(error => { this.failure = error; this.failed = true; }).finally(() => this.pending.delete(work)); return work;
  }
  async drain(timeoutMs: number): Promise<void> {
    this.draining = true; this.controller.abort();
    await bounded(Promise.allSettled([...this.pending]).then(() => {}), timeoutMs, 'Background tasks did not quiesce');
    if (this.failed) throw new PluginError('INTERNAL_ERROR', 'Tracked background task failed');
  }
}
// Account for the serialized size at enqueue time. Listeners share the
// validated payload and may mutate it before another subscriber dequeues it.
interface QueuedEvent { readonly payload: JsonValue; readonly bytes: number }
export async function bounded<T>(work: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([work, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new PluginError('DEADLINE_EXCEEDED', message)), Math.max(1, timeoutMs)); })]); }
  finally { if (timer) clearTimeout(timer); }
}
export class Dispatcher {
  readonly registry: PluginRegistry; private active = false; private idle = new Set<() => void>();
  async whenIdle(timeoutMs: number): Promise<void> { if (!this.active) return; let done!: () => void; try { await bounded(new Promise<void>(r => { done = r; this.idle.add(done); }), timeoutMs, 'Handler did not quiesce'); } finally { this.idle.delete(done); } }
  constructor(registry: PluginRegistry) { this.registry = registry; }
  async dispatch(params: JsonValue, context: CallContext): Promise<JsonValue> {
    const p = object(params); exactKeys(p, ['interface', 'version', 'method', 'input', 'context'], ['interface', 'version', 'method', 'input', 'context']);
    if (typeof p.interface !== 'string' || typeof p.method !== 'string') throw new PluginError('INVALID_ARGUMENT', 'Invalid invocation identity');
    const def = this.registry.definitions.get(p.interface);
    if (!def || def.contract.descriptor.version !== p.version) throw new PluginError('CONTRACT_MISMATCH', 'Unknown interface/version');
    const method = Object.hasOwn(def.contract.descriptor.methods, p.method) ? def.contract.descriptor.methods[p.method] : undefined;
    if (!method) throw new PluginError('METHOD_NOT_FOUND', `Unknown business method ${p.method}`);
    if (this.active) throw new PluginError('REENTRANT_CALL', 'Service endpoint already occupied');
    const data = validate(def.contract, method.input, p.input); this.active = true;
    try { return validate(def.contract, method.output, await def.handlers[p.method]!(data, context), 'INVALID_RESULT'); }
    catch (error) { throw validateDomainError(def.contract, p.method, error); }
    finally { this.active = false; for (const done of this.idle) done(); this.idle.clear(); }
  }
}
export interface PluginMetadata { id: string; version: string; requiresHost?: readonly Contract[] }
export interface ServeOptions { metadata: PluginMetadata; limits?: Partial<Limits> }
export async function serve(plugin: PluginDefinition | PluginRegistry, options: ServeOptions): Promise<void> {
  const env = process.env; const endpoint = env.NAPI_VM_PLUGIN_ENDPOINT, token = env.NAPI_VM_PLUGIN_TOKEN, instanceId = env.NAPI_VM_PLUGIN_INSTANCE_ID, sessionId = env.NAPI_VM_PLUGIN_SESSION_ID;
  if (!endpoint || !token || !instanceId || !sessionId) throw new PluginError('INVALID_ARGUMENT', 'Missing managed bootstrap environment. Start this plugin with napi-vm-plugin dev/invoke or TrustedPluginHost.load().');
  const match = /^127\.0\.0\.1:(\d{1,5})$/.exec(endpoint); if (!match || Number(match[1]) > 65535 || Number(match[1]) === 0) throw new PluginError('INVALID_ARGUMENT', 'Managed endpoint must be loopback IPv4:port');
  const socket = connect({ host: '127.0.0.1', port: Number(match[1]) }); const limits = effectiveLimits(options.limits);
  await bounded(new Promise<void>((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); }), limits.startupTimeoutMs, 'Connection startup timed out').catch(error => { socket.destroy(); throw error; });
  const peer = new RpcPeer(socket, { role: 'p', sessionId, limits, bootstrap: true });
  peer.holdEventsUntilReply('system.initialize');
  const r = registry(plugin), dispatch = new Dispatcher(r), tasks = new Tasks();
  let state: 'STARTING' | 'READY' | 'DRAINING' | 'STOPPING' | 'STOPPED' = 'STARTING'; let authenticated = false; let initialized = false; let sequence = 0n;
  let drained = false; let quiescence: Promise<void> | undefined, cleanup: Promise<void> | undefined; let lastHostSequence = 0n;
  const subscriptions = new Set<{ contract: Contract; event: string; listener: (payload: JsonValue) => void | Promise<void>; onError?: (error: PluginError) => void; queue: QueuedEvent[]; bytes: number; running: boolean; stopped: boolean }>();
  const contextFor = (request: RequestContext): CallContext => ({ ...request,
    invoke: (contract, method, input, callOptions = {}) => { if (!options.metadata.requiresHost?.some(c => c.descriptor.id === contract.descriptor.id && c.digest === contract.digest)) return Promise.reject(new PluginError('CONTRACT_MISMATCH', 'Host interface was not negotiated')); return invokeRemote(peer, contract, method, input, { ...callOptions, context: request }); },
    client(contract) { return createClient(contract, this); },
    async emit(contract, event, payload) {
      if (state !== 'READY' && !(state === 'DRAINING' && !drained)) throw new PluginError('NOT_READY', 'Events require an active or draining session');
      const eventType = Object.hasOwn(contract.descriptor.events, event) ? contract.descriptor.events[event] : undefined; if (!eventType || !r.definitions.has(contract.descriptor.id)) throw new PluginError('INVALID_ARGUMENT', 'Undeclared event');
      peer.notify('system.event', { interface: contract.descriptor.id, version: contract.descriptor.version, event, sessionId, sequence: String(++sequence), payload: validate(contract, eventType.payload, payload), relatedRequestId: request.id });
    },
    subscribe(contract, event, listener, onError) {
      if (!options.metadata.requiresHost?.some(c => c.descriptor.id === contract.descriptor.id && c.digest === contract.digest) || !contract.descriptor.events[event]) throw new PluginError('CONTRACT_MISMATCH', 'Host event was not negotiated');
      const sub = { contract, event, listener, ...(onError ? { onError } : {}), queue: [] as QueuedEvent[], bytes: 0, running: false, stopped: false }; subscriptions.add(sub); return () => { sub.stopped = true; sub.queue = []; sub.bytes = 0; subscriptions.delete(sub); };
    },
    log(...values) { console.log(...values); }, spawnTask(task) { return tasks.spawn(task); },
  });
  const synthetic = (): RequestContext => { const controller = new AbortController(); const end = performance.now() + limits.shutdownTimeoutMs; return { id: `p:${sessionId}:0`, signal: controller.signal, callChain: [peer.endpointId], remainingMs: () => Math.max(0, end - performance.now()), throwIfCancelled() { if (controller.signal.aborted) throw new PluginError('CANCELLED', 'Cancelled'); } }; };
  const stop = (context: CallContext): Promise<void> => {
    if (cleanup) return cleanup;
    state = 'STOPPING'; peer.cancelActive(context.id); for (const sub of subscriptions) { sub.stopped = true; sub.queue = []; sub.bytes = 0; } subscriptions.clear();
    cleanup = (async () => {
      try { await tasks.drain(limits.shutdownTimeoutMs); } catch { /* cleanup still runs after failed task drain */ }
      try { await bounded(Promise.all([...r.definitions.values()].map(d => d.hooks.shutdown?.(context))).then(() => {}), limits.shutdownTimeoutMs, 'Shutdown hook timed out'); }
      finally { state = 'STOPPED'; }
    })(); return cleanup;
  };
  peer.onRequest('system.initialize', async (params, request) => {
    if (!authenticated || initialized || state !== 'STARTING') throw new PluginError('NOT_READY', 'Invalid initialize transition'); initialized = true;
    const p = object(params); exactKeys(p, ['configuration', 'context', 'snapshot'], ['configuration', 'context', 'snapshot']);
    const snapshot = p.snapshot === null ? null : validateSnapshot(r.contracts, p.snapshot);
    const input: InitializeInput = { configuration: wireValue(p.configuration), context: wireValue(p.context), snapshot };
    for (const def of r.definitions.values()) await def.hooks.initialize?.(input, contextFor(request));
    state = 'READY'; return null;
  });
  peer.onRequest('system.invoke', (params, request) => {
    if (state !== 'READY') throw new PluginError('NOT_READY', 'Plugin is not ready'); return dispatch.dispatch(params, contextFor(request));
  });
  peer.onRequest('system.quiesce', async (params, request) => {
    const p = object(params); exactKeys(p, ['timeoutMs']); if (p.timeoutMs !== undefined && (!Number.isSafeInteger(p.timeoutMs) || (p.timeoutMs as number) <= 0)) throw new PluginError('INVALID_ARGUMENT', 'quiesce timeoutMs must be a positive safe integer');
    if (state !== 'READY' && state !== 'DRAINING') throw new PluginError('NOT_READY', 'Cannot quiesce before initialization');
    state = 'DRAINING';
    if (!quiescence) {
      drained = false;
      const timeout = typeof p.timeoutMs === 'number' ? Math.min(p.timeoutMs, limits.shutdownTimeoutMs) : limits.shutdownTimeoutMs;
      quiescence = (async () => { await peer.whenIdle(timeout); await tasks.drain(timeout); for (const def of r.definitions.values()) await def.hooks.quiesce?.(contextFor(request)); drained = true; })().catch(error => { quiescence = undefined; throw error; });
    }
    await quiescence; return null;
  });
  peer.onRequest('system.snapshot', async (params, request) => {
    exactKeys(object(params), []);
    if (state !== 'DRAINING' || !quiescence || peer.busy || tasks.pending.size) throw new PluginError('NOT_READY', 'Snapshot requires successful quiescence'); await quiescence;
    const stateful = [...r.definitions.values()].filter(d => d.contract.descriptor.state);
    if (!stateful.length) return null;
    if (stateful.length > 1) throw new PluginError('STATE_INCOMPATIBLE', 'v1 supports one state contract per plugin');
    const def = stateful[0]!; if (!def.hooks.snapshot) throw new PluginError('STATE_INCOMPATIBLE', 'Missing snapshot hook');
    const snapshot = validateSnapshot(r.contracts, await def.hooks.snapshot(contextFor(request))); return wireValue(snapshot);
  });
  peer.onRequest('system.shutdown', async (params, request) => { exactKeys(object(params), []); try { await stop(contextFor(request)); return null; } finally { setTimeout(() => { peer.close(); process.exit(0); }, 20); } });
  peer.onRequest('system.ping', params => { exactKeys(object(params), []); return null; });
  peer.on('event', (value: unknown) => {
    try {
      if (state !== 'READY') throw new PluginError('NOT_READY', 'Host events require READY');
      const e = object(value); exactKeys(e, ['interface', 'version', 'event', 'sessionId', 'sequence', 'payload', 'relatedRequestId'], ['interface', 'version', 'event', 'sessionId', 'sequence', 'payload']);
      if (e.sessionId !== sessionId || typeof e.sequence !== 'string' || !/^[1-9][0-9]{0,30}$/.test(e.sequence) || BigInt(e.sequence) <= lastHostSequence) throw new PluginError('INVALID_REQUEST', 'Invalid host event session/sequence');
      const c = options.metadata.requiresHost?.find(c => c.descriptor.id === e.interface && c.descriptor.version === e.version); if (!c || typeof e.event !== 'string' || !c.descriptor.events[e.event]) throw new PluginError('CONTRACT_MISMATCH', 'Undeclared host event');
      const payload = validate(c, c.descriptor.events[e.event]!.payload, e.payload); const size = Buffer.byteLength(JSON.stringify(payload)); lastHostSequence = BigInt(e.sequence);
      const queued = { payload, bytes: size };
      for (const sub of subscriptions) if (!sub.stopped && sub.contract.descriptor.id === e.interface && sub.event === e.event) {
        if (sub.queue.length >= peer.limits.maxEventQueue || sub.bytes + size > peer.limits.maxQueuedBytes) { sub.stopped = true; sub.queue = []; sub.bytes = 0; subscriptions.delete(sub); try { sub.onError?.(new PluginError('OVERLOADED', 'Host event subscriber queue overflow')); } catch {} continue; }
        sub.queue.push(queued); sub.bytes += size; if (!sub.running) { sub.running = true; void tasks.spawn(async () => { await new Promise<void>(resolve => setImmediate(resolve)); try { while (!sub.stopped && sub.queue.length) { const next = sub.queue.shift()!; sub.bytes -= next.bytes; await sub.listener(next.payload); } } catch (error) { sub.stopped = true; sub.queue = []; sub.bytes = 0; subscriptions.delete(sub); try { sub.onError?.(asPluginError(error)); } catch {} } finally { sub.running = false; } }); }
      }
    } catch (error) { peer.close(asPluginError(error)); }
  });
  const closed = new Promise<void>(resolve => peer.once('closed', () => { void bounded(stop(contextFor(synthetic())), limits.shutdownTimeoutMs, 'Disconnect cleanup deadline').catch(() => {}).finally(() => { resolve(); setImmediate(() => process.exit(0)); }); }));
  try {
    const hello = object(await peer.request('system.hello', { token, instanceId, sessionId, pluginId: options.metadata.id, pluginVersion: options.metadata.version, protocol: { major: 1, minMinor: 0, maxMinor: 0 }, interfaces: identities(r.contracts), requiresHost: identities(options.metadata.requiresHost ?? []), extensions: [] }, { timeoutMs: limits.startupTimeoutMs }));
    exactKeys(hello, ['protocol', 'interfaces', 'limits', 'extensions'], ['protocol', 'interfaces', 'limits', 'extensions']);
    const protocol = object(hello.protocol); exactKeys(protocol, ['major', 'minor'], ['major', 'minor']); if (protocol.major !== 1 || protocol.minor !== 0) throw new PluginError('PROTOCOL_MISMATCH', 'Host selected unsupported protocol');
    const negotiatedLimits = object(hello.limits); exactKeys(negotiatedLimits, Object.keys(DEFAULT_LIMITS));
    if (!Array.isArray(hello.extensions) || hello.extensions.length !== 0) throw new PluginError('PROTOCOL_MISMATCH', 'Host selected an unsupported extension');
    // Hosts may advertise more services than this plugin requires.
    const offered = object(hello.interfaces); const required = identities(options.metadata.requiresHost ?? []);
    for (const [id, info] of Object.entries(required)) matchIdentities({ [id]: info }, { [id]: offered[id] });
    peer.negotiate(object(hello.limits) as Partial<Limits>); authenticated = true;
    await closed;
  } catch (error) { peer.close(asPluginError(error)); await stop(contextFor(synthetic())).catch(() => {}); throw error; }
  finally { socket.destroy(); }
}
export interface HarnessOptions { services?: PluginDefinition[]; configuration?: JsonValue; context?: JsonValue; snapshot?: Snapshot | null }
export interface Harness extends Invoker { events: JsonValue[]; client(contract: Contract): ReturnType<typeof createClient>; emitHostEvent(contract: Contract, event: string, payload: unknown): Promise<void>; snapshot(): Promise<Snapshot | null>; shutdown(): Promise<void> }
export async function createHarness(plugin: PluginDefinition | PluginRegistry, options: HarnessOptions = {}): Promise<Harness> {
  let drained = false;
  const r = registry(plugin); const dispatcher = new Dispatcher(r); const serviceRegistry = new PluginRegistry(); options.services?.forEach(d => serviceRegistry.add(d)); const services = new Dispatcher(serviceRegistry); const tasks = new Tasks();
  let state = 'STARTING'; let count = 0; const controller = new AbortController(); const events: JsonValue[] = []; const activeControllers = new Set<AbortController>();
  const hostSubscriptions = new Set<{ contract: Contract; event: string; listener: (payload: JsonValue) => void | Promise<void>; onError?: (error: PluginError) => void; queue: QueuedEvent[]; bytes: number; running: boolean; stopped: boolean }>();
  function context(chain: string[], signal = controller.signal, deadline = performance.now() + 30000): CallContext {
    const ctx: CallContext = { id: `h:harness:${++count}`, signal, callChain: chain, remainingMs: () => Math.max(0, deadline - performance.now()), throwIfCancelled() { if (signal.aborted) throw new PluginError('CANCELLED', 'Cancelled'); },
      async invoke(contract, method, input) { if (chain.includes('h:harness')) throw new PluginError('REENTRANT_CALL', 'Cyclic harness callback'); const p = { interface: contract.descriptor.id, version: contract.descriptor.version, method, input: wireValue(input), context: { timeoutMs: ctx.remainingMs(), callChain: chain } }; return services.dispatch(p, context([...chain, 'h:harness'], signal, deadline)); },
      client(contract) { return createClient(contract, this); }, subscribe(contract, event, listener, onError) { if (serviceRegistry.definitions.get(contract.descriptor.id)?.contract.digest !== contract.digest || !Object.hasOwn(contract.descriptor.events, event)) throw new PluginError('CONTRACT_MISMATCH', 'Harness event service not supplied'); const sub = { contract, event, listener, ...(onError ? { onError } : {}), queue: [] as QueuedEvent[], bytes: 0, running: false, stopped: false }; hostSubscriptions.add(sub); return () => { sub.stopped = true; sub.queue = []; sub.bytes = 0; hostSubscriptions.delete(sub); }; }, async emit(contract, event, payload) { if (state !== 'READY' && !(state === 'DRAINING' && !drained)) throw new PluginError('NOT_READY', 'Events require an active or draining session'); const type = Object.hasOwn(contract.descriptor.events, event) ? contract.descriptor.events[event] : undefined; if (!type) throw new PluginError('INVALID_ARGUMENT', 'Unknown event'); events.push({ event, payload: validate(contract, type.payload, payload) }); }, log(...values) { console.log(...values); }, spawnTask(task) { return tasks.spawn(task); },
    }; return ctx;
  }
  if (options.snapshot) validateSnapshot(r.contracts, options.snapshot);
  try { for (const d of r.definitions.values()) await bounded(Promise.resolve(d.hooks.initialize?.({ configuration: wireValue(options.configuration === undefined ? {} : options.configuration), context: wireValue(options.context === undefined ? {} : options.context), snapshot: options.snapshot ?? null }, context(['p:harness']))), 30000, 'Harness initialization timed out'); state = 'READY'; } catch (error) { state = 'STOPPED'; controller.abort(); await tasks.drain(5000).catch(() => {}); for (const d of r.definitions.values()) await bounded(Promise.resolve().then(() => d.hooks.shutdown?.(context(['p:harness']))), 5000, 'Harness cleanup timed out').catch(() => {}); throw error; }
  const harness: Harness = {
    events, client(contract) { return createClient(contract, this); },
    async emitHostEvent(contract, event, payload) {
      if (state !== 'READY') throw new PluginError('NOT_READY', 'Harness is not ready');
      if (serviceRegistry.definitions.get(contract.descriptor.id)?.contract.digest !== contract.digest || !Object.hasOwn(contract.descriptor.events, event)) throw new PluginError('CONTRACT_MISMATCH', 'Harness event service not supplied');
      const data = validate(contract, contract.descriptor.events[event]!.payload, payload); const size = Buffer.byteLength(JSON.stringify(data));
      const queued = { payload: data, bytes: size };
      for (const sub of hostSubscriptions) if (!sub.stopped && sub.contract.descriptor.id === contract.descriptor.id && sub.event === event) {
        if (sub.queue.length >= 256 || sub.bytes + size > 16 * 1024 * 1024) { sub.stopped = true; sub.queue = []; sub.bytes = 0; hostSubscriptions.delete(sub); try { sub.onError?.(new PluginError('OVERLOADED', 'Harness subscriber overflow')); } catch {} continue; }
        sub.queue.push(queued); sub.bytes += size;
        if (!sub.running) { sub.running = true; void tasks.spawn(async () => { await new Promise<void>(resolve => setImmediate(resolve)); try { while (!sub.stopped && sub.queue.length) { const next = sub.queue.shift()!; sub.bytes -= next.bytes; await sub.listener(next.payload); } } catch (error) { sub.stopped = true; sub.queue = []; sub.bytes = 0; hostSubscriptions.delete(sub); try { sub.onError?.(asPluginError(error)); } catch {} } finally { sub.running = false; } }); }
      }
    },
    async invoke(contract, method, input, callOptions = {}) {
      if (state !== 'READY') throw new PluginError('NOT_READY', 'Harness not ready');
      if (callOptions.context?.callChain.includes('p:harness')) throw new PluginError('REENTRANT_CALL', 'Cyclic harness invocation');
      const own = new AbortController(); const signal = callOptions.signal ?? callOptions.context?.signal;
      if (signal?.aborted) throw new PluginError('CANCELLED', 'Harness call cancelled before dispatch');
      const cancel = () => own.abort(new PluginError('CANCELLED', 'Harness call cancelled; handler may still run'));
      const timeout = Math.min(callOptions.timeoutMs ?? 30000, callOptions.context?.remainingMs?.() ?? 30000); if (!Number.isFinite(timeout) || timeout <= 0) throw new PluginError('INVALID_ARGUMENT', 'Invalid harness timeout');
      activeControllers.add(own); signal?.addEventListener('abort', cancel, { once: true });
      const timer = setTimeout(() => own.abort(new PluginError('DEADLINE_EXCEEDED', 'Harness deadline exceeded; handler may still run')), timeout);
      let abort!: () => void; const cancelled = new Promise<never>((_, reject) => { abort = () => reject(own.signal.reason); own.signal.addEventListener('abort', abort, { once: true }); });
      try { return await Promise.race([dispatcher.dispatch({ interface: contract.descriptor.id, version: contract.descriptor.version, method, input: wireValue(input), context: { timeoutMs: timeout, callChain: [] } }, context(['p:harness'], own.signal, performance.now() + timeout)), cancelled]); }
      finally { activeControllers.delete(own); clearTimeout(timer); signal?.removeEventListener('abort', cancel); own.signal.removeEventListener('abort', abort); }
    },
    async snapshot() { state = 'DRAINING'; await dispatcher.whenIdle(5000); await tasks.drain(5000); for (const d of r.definitions.values()) await d.hooks.quiesce?.(context(['p:harness'])); drained = true; const stateful = [...r.definitions.values()].filter(d => d.contract.descriptor.state); if (stateful.length > 1) throw new PluginError('STATE_INCOMPATIBLE', 'v1 supports one state contract per plugin'); if (!stateful.length) return null; return validateSnapshot(r.contracts, await stateful[0]!.hooks.snapshot?.(context(['p:harness']))); },
    async shutdown() { if (state === 'STOPPED') return; state = 'STOPPED'; controller.abort(); for (const active of activeControllers) active.abort(new PluginError('CANCELLED', 'Harness shutdown')); for (const sub of hostSubscriptions) { sub.stopped = true; sub.queue = []; sub.bytes = 0; } hostSubscriptions.clear(); try { await dispatcher.whenIdle(5000); await tasks.drain(5000); } finally { for (const d of r.definitions.values()) await d.hooks.shutdown?.(context(['p:harness'])); } },
  }; return harness;
}
