import { createServer, type Server, type Socket } from 'node:net';
import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { randomBytes, randomUUID, createHash, timingSafeEqual } from 'node:crypto';
import { readFile, realpath, stat, access } from 'node:fs/promises';
import { constants } from 'node:fs';
import { resolve, dirname, relative, isAbsolute, delimiter, extname } from 'node:path';
import { EventEmitter } from 'node:events';
import { Contract, JsonValue, JsonObject, PluginError, RpcPeer, Limits, CallOptions, effectiveLimits, validate, wireValue, object, exactKeys, identifier, version, identities, matchIdentities, asPluginError, RequestContext, assertContract } from '@napi-vm/plugin-protocol';
import { definePlugin, PluginRegistry, Dispatcher, Handler, CallContext, Snapshot, createClient, invokeRemote, validateSnapshot, bounded } from '@napi-vm/plugin-sdk';
export type Status = 'DISCOVERED' | 'STARTING' | 'READY' | 'DRAINING' | 'STOPPING' | 'STOPPED' | 'FAILED';
export interface Target { os: string; arch: string; libc?: string }
export interface NativeDependency { name: string; path: string; target: Target; abi: string; runtime: 'node' | 'bun'; status: 'tested' | 'buildable' | 'distributed' }
export interface ServiceDependency { name: string; kind: 'http' | 'mcp'; owner?: 'external' | 'host'; readiness?: { path: string; status: number }; shutdown?: 'terminate' }
export interface Manifest {
  manifestVersion: 2; execution: 'trusted-process'; id: string; version: string;
  protocol: { major: number; minMinor: number; maxMinor: number };
  provides: Record<string, string>; requiresHost: Record<string, string>;
  profile?: 'portable-js' | 'external-runtime' | 'native-executable'; contracts?: string[];
  launch: { kind: 'javascript'; entry: string; runtimes: ('node' | 'bun')[]; preferredRuntime: 'node' | 'bun'; args?: string[]; versions?: Partial<Record<'node' | 'bun', string>> } | { kind: 'executable'; entry: string; args?: string[]; target: Target };
  development?: { entry: string; runtime: 'node' | 'bun' }; assets?: string[];
  dependencies?: { native: NativeDependency[]; services: ServiceDependency[]; capabilities: string[] };
  extensions?: Record<string, unknown>;
}
export interface PackageLock { lockVersion: 1; pluginId: string; pluginVersion: string; artifact: { profile: string; target: Target; runtime: string[]; abi: string; availability: { built: boolean; distributed: boolean; tested: string[] } }; interfaces: Record<string, { version: string; digest: string }>; files: Record<string, string> }
export interface ResolvedManifest { manifest: Manifest; path: string; directory: string; contracts: Contract[]; lock?: PackageLock }
const check = (condition: unknown, message: string): void => { if (!condition) throw new PluginError('INVALID_ARGUMENT', message); };
const boundedString = (value: unknown, max = 4096): value is string => typeof value === 'string' && value.length > 0 && value.length <= max;
export function validateManifest(value: unknown): Manifest {
  const p = object(wireValue(value), 'manifest');
  if (p.manifestVersion !== 2 || p.execution !== 'trusted-process') throw new PluginError('INVALID_ARGUMENT', 'Wrong manifest format: use manifestVersion 2 / trusted-process. Existing VM/sandbox plugin.json files require the legacy host.');
  exactKeys(p, ['manifestVersion', 'execution', 'id', 'version', 'protocol', 'provides', 'requiresHost', 'launch', 'development', 'assets', 'extensions', 'contracts', 'profile', 'dependencies'], ['id', 'version', 'protocol', 'provides', 'requiresHost', 'launch']);
  check(identifier(p.id) && version(p.version), 'Invalid plugin identifier/version');
  const protocol = object(p.protocol); exactKeys(protocol, ['major', 'minMinor', 'maxMinor'], ['major', 'minMinor', 'maxMinor']);
  check([protocol.major, protocol.minMinor, protocol.maxMinor].every(v => Number.isSafeInteger(v) && (v as number) >= 0) && (protocol.minMinor as number) <= (protocol.maxMinor as number), 'Invalid protocol range');
  for (const key of ['provides', 'requiresHost']) { const interfaces = object(p[key]); check(Object.keys(interfaces).length <= 64, 'Too many interfaces'); for (const [id, v] of Object.entries(interfaces)) check(identifier(id) && version(v), `Invalid interface ${id}`); }
  const launch = object(p.launch); check(launch.kind === 'javascript' || launch.kind === 'executable', 'Unknown launch kind');
  exactKeys(launch, launch.kind === 'javascript' ? ['kind', 'entry', 'runtimes', 'preferredRuntime', 'args', 'versions'] : ['kind', 'entry', 'args', 'target'], ['kind', 'entry']);
  check(boundedString(launch.entry), 'Invalid launch entry');
  if (launch.args !== undefined) check(Array.isArray(launch.args) && launch.args.length <= 64 && launch.args.every(a => typeof a === 'string' && a.length <= 4096), 'Invalid launch args');
  if (launch.kind === 'javascript') {
    check(Array.isArray(launch.runtimes) && launch.runtimes.length >= 1 && launch.runtimes.length <= 2 && new Set(launch.runtimes).size === launch.runtimes.length && launch.runtimes.every(r => r === 'node' || r === 'bun'), 'Invalid/duplicate runtimes');
    check((launch.runtimes as unknown[]).includes(launch.preferredRuntime), 'Preferred runtime must be declared');
    if (launch.versions !== undefined) { const versions = object(launch.versions); exactKeys(versions, ['node', 'bun']); for (const v of Object.values(versions)) check(typeof v === 'string' && /^(>=)?\d+\.\d+\.\d+$/.test(v), 'Runtime versions support exact or >=major.minor.patch'); }
  } else validateTarget(launch.target);
  if (p.development !== undefined) { const dev = object(p.development); exactKeys(dev, ['entry', 'runtime'], ['entry', 'runtime']); check(boundedString(dev.entry) && ['node', 'bun'].includes(dev.runtime as string), 'Invalid development entry'); }
  for (const key of ['assets', 'contracts']) if (p[key] !== undefined) check(Array.isArray(p[key]) && (p[key] as unknown[]).length <= (key === 'contracts' ? 64 : 1024) && (p[key] as unknown[]).every(v => boundedString(v)), `Invalid ${key}`);
  if (p.profile !== undefined) check(p.profile === 'portable-js' || p.profile === 'external-runtime' || p.profile === 'native-executable' && launch.kind === 'executable', 'Unknown dependency profile');
  if (p.dependencies !== undefined) {
    const dependencies = object(p.dependencies); exactKeys(dependencies, ['native', 'services', 'capabilities'], ['native', 'services', 'capabilities']);
    for (const key of ['native', 'services', 'capabilities']) check(Array.isArray(dependencies[key]) && (dependencies[key] as unknown[]).length <= 64, `Invalid dependency ${key}`);
    for (const raw of dependencies.native as unknown[]) {
      const dep = object(raw); exactKeys(dep, ['name', 'path', 'target', 'abi', 'runtime', 'status'], ['name', 'path', 'target', 'abi', 'runtime', 'status']);
      check(identifier(dep.name) && boundedString(dep.path) && boundedString(dep.abi, 128) && ['node', 'bun'].includes(dep.runtime as string) && ['tested', 'buildable', 'distributed'].includes(dep.status as string), 'Invalid native dependency'); validateTarget(dep.target);
    }
    check(p.profile !== 'portable-js' || !(dependencies.native as unknown[]).length, 'portable-js cannot contain native dependencies');
    for (const raw of dependencies.services as unknown[]) { const dep = object(raw); exactKeys(dep, ['name', 'kind', 'owner', 'readiness', 'shutdown'], ['name', 'kind']); check(identifier(dep.name) && ['http', 'mcp'].includes(dep.kind as string) && (dep.owner === undefined || ['external', 'host'].includes(dep.owner as string)), 'Invalid service dependency'); if (dep.owner === 'host') { const readiness = object(dep.readiness); exactKeys(readiness, ['path', 'status'], ['path', 'status']); check(boundedString(readiness.path, 1024) && (readiness.path as string).startsWith('/') && Number.isInteger(readiness.status) && (readiness.status as number) >= 100 && (readiness.status as number) <= 599 && dep.shutdown === 'terminate', 'Host-owned services require HTTP readiness and termination ownership'); } else check(dep.readiness === undefined && dep.shutdown === undefined, 'External services cannot declare host cleanup'); }
    check((dependencies.capabilities as unknown[]).every(identifier), 'Invalid capability dependency');
    const names = [...(dependencies.native as { name: string }[]), ...(dependencies.services as { name: string }[])].map(d => d.name); check(new Set(names).size === names.length, 'Duplicate dependency name');
  }
  if (p.extensions !== undefined) { const extensions = object(p.extensions); for (const key of Object.keys(extensions)) check(identifier(key) && key.includes('.'), 'Extension keys must be namespaced'); }
  return p as unknown as Manifest;
}
function validateTarget(value: unknown): void { const t = object(value); exactKeys(t, ['os', 'arch', 'libc'], ['os', 'arch']); check(['linux', 'darwin', 'win32'].includes(t.os as string) && ['x64', 'arm64'].includes(t.arch as string) && (t.libc === undefined || t.os === 'linux' && ['gnu', 'musl'].includes(t.libc as string)), 'Invalid platform target'); }
export async function packagePath(directory: string, entry: string): Promise<string> {
  check(boundedString(entry) && !isAbsolute(entry) && !/^[A-Za-z]:|^[/\\]/.test(entry), 'Package paths must be relative');
  const normalized = entry.replace(/\\/g, '/'); check(!normalized.split('/').includes('..'), 'Package path traversal');
  const base = await realpath(directory), path = await realpath(resolve(base, normalized)); const rel = relative(base, path);
  check(rel !== '' && !rel.startsWith('..') && !isAbsolute(rel), `Package path escapes package: ${entry}`); check((await stat(path)).isFile(), `Package file not found: ${entry}`); return path;
}
async function readJsonBounded(path: string, maxBytes: number): Promise<unknown> { const info = await stat(path); check(info.isFile() && info.size > 0 && info.size <= maxBytes, 'Metadata file exceeds its size limit'); return JSON.parse(await readFile(path, 'utf8')) as unknown; }
export async function readManifest(path: string, options: { verifyIntegrity?: boolean } = {}): Promise<ResolvedManifest> {
  let absolute = resolve(path); if ((await stat(absolute)).isDirectory()) absolute = resolve(absolute, 'plugin.json');
  const directory = dirname(absolute); const manifest = validateManifest(await readJsonBounded(absolute, 65536));
  await packagePath(directory, manifest.launch.entry); for (const file of manifest.assets ?? []) await packagePath(directory, file);
  const contracts: Contract[] = [];
  for (const entry of manifest.contracts ?? []) {
    const contract = await readJsonBounded(await packagePath(directory, entry), 4 * 1024 * 1024) as Contract;
    assertContract(contract);
    check(!contracts.some(c => c.descriptor.id === contract.descriptor.id), 'Duplicate contract artifact'); contracts.push(contract);
  }
  for (const dep of manifest.dependencies?.native ?? []) await packagePath(directory, dep.path);
  let lock: PackageLock | undefined;
  try { lock = await readJsonBounded(resolve(directory, 'plugin.lock.json'), 4 * 1024 * 1024) as PackageLock; }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  if (lock && options.verifyIntegrity !== false) {
    check(lock.lockVersion === 1 && lock.pluginId === manifest.id && lock.pluginVersion === manifest.version, 'Package lock identity mismatch');
    const artifact = object(lock.artifact); exactKeys(artifact, ['profile', 'target', 'runtime', 'abi', 'availability'], ['profile', 'target', 'runtime', 'abi', 'availability']);
    check(artifact.profile === (manifest.profile ?? (manifest.launch.kind === 'javascript' ? 'external-runtime' : 'native-executable')), 'Package profile mismatch');
    const target = object(artifact.target); exactKeys(target, ['os', 'arch', 'libc'], ['os', 'arch']);
    if (target.os !== 'any' || target.arch !== 'any') validateTarget(target);
    check(Array.isArray(artifact.runtime) && artifact.runtime.length <= 2 && artifact.runtime.every(r => ['node', 'bun'].includes(r as string)) && new Set(artifact.runtime).size === artifact.runtime.length, 'Invalid artifact runtime metadata');
    check(boundedString(artifact.abi, 128), 'Invalid artifact ABI'); const availability = object(artifact.availability); exactKeys(availability, ['built', 'distributed', 'tested'], ['built', 'distributed', 'tested']);
    check(availability.built === true && typeof availability.distributed === 'boolean' && Array.isArray(availability.tested) && availability.tested.length <= 256 && availability.tested.every(x => typeof x === 'string' && x.length <= 256), 'Invalid artifact availability metadata');
    if (manifest.launch.kind === 'javascript') { check(artifact.abi === 'javascript-esm' && JSON.stringify([...artifact.runtime as string[]].sort()) === JSON.stringify([...manifest.launch.runtimes].sort()), 'JavaScript artifact runtime/ABI mismatch'); }
    else check(JSON.stringify(target) === JSON.stringify(manifest.launch.target), 'Executable artifact target mismatch');
    matchIdentities(identities(contracts), lock.interfaces);
    const required = ['plugin.json', manifest.launch.entry, ...(manifest.assets ?? []), ...(manifest.contracts ?? []), ...(manifest.dependencies?.native ?? []).map(d => d.path)].map(p => p.replace(/^\.\//, '').replace(/\\/g, '/'));
    const files = object(lock.files); check(Object.keys(files).length <= 10000, 'Package inventory too large');
    for (const file of required) check(Object.hasOwn(files, file), `Package inventory missing ${file}`);
    for (const [file, checksum] of Object.entries(files)) { check(typeof checksum === 'string' && /^[a-f0-9]{64}$/.test(checksum), 'Invalid package checksum'); const contents = await readFile(await packagePath(directory, file)); check(createHash('sha256').update(contents).digest('hex') === checksum, `Package integrity failure: ${file}`); }
  }
  return { manifest, path: absolute, directory, contracts, ...(lock ? { lock } : {}) };
}
export interface LaunchOptions { runtime?: 'node' | 'bun'; runtimePaths?: Partial<Record<'node' | 'bun', string>>; development?: boolean }
export interface ResolvedLaunch { command: string; args: string[]; cwd: string; runtime: 'node' | 'bun'; entry: string; version: string }
async function findExecutable(name: string): Promise<string | undefined> {
  const names = process.platform === 'win32' && !extname(name) ? [name + '.exe', name + '.cmd', name] : [name];
  for (const dir of isAbsolute(name) || name.includes('/') || name.includes('\\') ? [''] : (process.env.PATH ?? '').split(delimiter)) for (const candidate of names) {
    const path = resolve(dir, candidate); try { await access(path, constants.X_OK); if ((await stat(path)).isFile()) return path; } catch { /* try declared runtime alternatives before spawning */ }
  } return undefined;
}
// Version discovery must not stall RPC deadlines, audio or event delivery.
function probeRuntime(command: string, args: string[]): Promise<{ stdout: string; status: number; error?: Error }> {
  return new Promise(resolve => {
    execFile(command, args, { encoding: 'utf8', timeout: 5000, maxBuffer: 8192, shell: false }, (error, stdout) => {
      resolve({ stdout, status: error ? 1 : 0, ...(error ? { error } : {}) });
    });
  });
}
function versionAtLeast(actual: string, requirement: string): boolean { const a = actual.replace(/^v/, '').split('.').map(Number), b = requirement.replace(/^>=/, '').split('.').map(Number); if (!requirement.startsWith('>=')) return a.slice(0, 3).join('.') === b.join('.'); for (let i = 0; i < 3; i++) { if (a[i]! > b[i]!) return true; if (a[i]! < b[i]!) return false; } return true; }
function targetMatches(target: Target): boolean { const libc = process.platform === 'linux' ? ((process.report?.getReport() as { header?: { glibcVersionRuntime?: string } })?.header?.glibcVersionRuntime ? 'gnu' : 'musl') : undefined; return target.os === process.platform && target.arch === process.arch && (!target.libc || target.libc === libc); }
export async function resolveLaunch(resolved: ResolvedManifest, options: LaunchOptions = {}): Promise<ResolvedLaunch> {
  const m = resolved.manifest;
  if (m.launch.kind !== 'javascript') throw new PluginError('INVALID_ARGUMENT', 'The TypeScript host supports JavaScript/TypeScript artifacts only. Use the Rust host for native or standalone executable artifacts.');
  check(m.protocol.major === 1 && m.protocol.minMinor <= 0 && m.protocol.maxMinor >= 0, 'Unsupported plugin protocol range');
  const dev = options.development && m.development ? m.development : undefined;
  const runtime = options.runtime ?? dev?.runtime;
  if (runtime && !m.launch.runtimes.includes(runtime)) throw new PluginError('INVALID_ARGUMENT', `Runtime ${runtime} is not declared by this plugin`);
  const preferred = m.launch.preferredRuntime;
  const candidates = runtime ? [runtime] : [preferred, ...m.launch.runtimes.filter(r => r !== preferred)];
  const failures: string[] = [];
  for (const name of candidates) {
    const command = await findExecutable(options.runtimePaths?.[name] ?? name);
    if (!command) { failures.push(`${name}: executable unavailable`); continue; }
    const probe = await probeRuntime(command, ['--version']);
    const actual = probe.stdout?.trim().replace(/^v/, '') ?? '';
    if (probe.status !== 0 || !/^\d+\.\d+\.\d+/.test(actual) || m.launch.versions?.[name] && !versionAtLeast(actual, m.launch.versions[name]!)) { failures.push(`${name}: version requirement failed`); continue; }
    const entry = await packagePath(resolved.directory, name === 'bun' ? dev?.entry ?? m.launch.entry : m.launch.entry);
    if (name === 'node' && /\.[cm]?tsx?$/.test(entry)) throw new PluginError('INVALID_ARGUMENT', 'Node requires emitted JavaScript; build the TypeScript artifact first');
    if (!options.development && /\.[cm]?tsx?$/.test(entry)) throw new PluginError('INVALID_ARGUMENT', 'Production requires emitted JavaScript');
    const versionsProbe = await probeRuntime(command, ['-e', 'process.stdout.write(JSON.stringify(process.versions))']);
    let selectedVersions: Record<string, string> = {}; try { if (versionsProbe.status === 0) selectedVersions = JSON.parse(versionsProbe.stdout) as Record<string, string>; } catch {}
    if (versionsProbe.status !== 0) { failures.push(`${name}: runtime inspection failed (${versionsProbe.error?.message ?? versionsProbe.status})`); continue; }
    if (!selectedVersions.node || (name === 'bun' ? !selectedVersions.bun : !!selectedVersions.bun)) { failures.push(`${name}: configured executable is the wrong runtime`); continue; }
    for (const dep of m.dependencies?.native ?? []) {
      check(m.profile === 'external-runtime', 'Native dependencies require external-runtime profile');
      check(dep.runtime === name && targetMatches(dep.target) && dep.status === 'tested', `Native dependency ${dep.name} is not runtime-tested for this target/runtime`);
      check(/^napi-[1-9]\d*$/.test(dep.abi), `Native dependency ${dep.name} must declare a Node-API ABI`);
      check(Number(dep.abi.slice(5)) <= Number(selectedVersions.napi ?? 0), `Native dependency ${dep.name} requires newer Node-API`);
    }
    return { command, args: [...(name === 'bun' ? ['--no-install'] : []), entry, ...(m.launch.args ?? [])], cwd: resolved.directory, runtime: name, entry, version: actual };
  }
  throw new PluginError('INVALID_ARGUMENT', `No compatible declared runtime is available (${failures.join('; ')}). Install/configure it explicitly; load never installs dependencies.`);
}
export interface InjectedService { url: string; headers?: Record<string, string> }
export interface LoadOptions extends LaunchOptions { configuration?: JsonValue; context?: JsonValue; env?: Record<string, string>; services?: Record<string, InjectedService>; instanceId?: string }
export interface HostOptions { contracts?: Contract[]; runtimePaths?: Partial<Record<'node' | 'bun', string>>; limits?: Partial<Limits> }
interface Subscription { contract: Contract; event: string; listener: (payload: JsonValue) => void | Promise<void>; onError?: (error: PluginError) => void; queue: { payload: JsonValue; sessionId: string; bytes: number }[]; bytes: number; running: boolean; stopped: boolean }
const errorSecrets = new WeakMap<PluginHandle, string[]>();
/** Capture the current generation's secrets before awaiting plugin work. */
function errorSanitizer(handle: PluginHandle): (error: unknown) => PluginError {
  const secrets = errorSecrets.get(handle) ?? [];
  const text = (value: string): string => { for (const secret of secrets) value = value.split(secret).join('[REDACTED]'); return value; };
  const data = (value: JsonValue): JsonValue => {
    if (typeof value === 'string') return text(value);
    if (Array.isArray(value)) return value.map(data);
    if (value && typeof value === 'object') { const out: JsonObject = Object.create(null) as JsonObject; for (const [key, item] of Object.entries(value)) out[text(key)] = data(item); return out; }
    return value;
  };
  return error => { const failure = asPluginError(error); return new PluginError(failure.code, text(failure.message), failure.details === undefined ? undefined : data(failure.details), failure.domainCode, failure.data === undefined ? undefined : data(failure.data)); };
}
export class PluginHandle {
  status: Status = 'DISCOVERED'; sessionId = ''; selectedRuntime: ResolvedLaunch | undefined; metadata: Manifest;
  readonly instanceId: string; readonly subscriptions = new Set<Subscription>(); readonly logs: { stream: 'stdout' | 'stderr'; text: string }[] = []; droppedLogBytes = 0; logBytes = 0; discardedStaleEvents = 0;
  peer: RpcPeer | undefined; child: ChildProcess | undefined; snapshot: Snapshot | null | undefined; lastEvent = 0n; outgoingEvent = 0n; exitPromise: Promise<void> | undefined;
  resolved: ResolvedManifest; options: LoadOptions; private readonly host: TrustedPluginHost;
  constructor(host: TrustedPluginHost, resolved: ResolvedManifest, options: LoadOptions) { this.host = host; this.resolved = resolved; this.metadata = resolved.manifest; this.options = options; Object.defineProperty(this, 'options', { enumerable: false }); this.instanceId = options.instanceId ?? randomUUID(); }
  invoke(contract: Contract, method: string, input: unknown, options?: CallOptions): Promise<JsonValue> { if (options?.context?.callChain.includes(`p:${this.sessionId}`)) return Promise.reject(new PluginError('REENTRANT_CALL', 'Target is active in call chain')); if (this.status !== 'READY' || !this.peer) return Promise.reject(new PluginError('NOT_READY', `Plugin is ${this.status}`)); this.host.assertContract(this, contract); const sanitize = errorSanitizer(this); return invokeRemote(this.peer, contract, method, input, options).catch(error => { throw sanitize(error); }); }
  client(contract: Contract): ReturnType<typeof createClient> { this.host.assertContract(this, contract); return createClient(contract, this); }
  subscribe(contract: Contract, event: string, listener: Subscription['listener'], onError?: Subscription['onError']): () => void {
    this.host.assertContract(this, contract); if (!Object.hasOwn(contract.descriptor.events, event)) throw new PluginError('INVALID_ARGUMENT', 'Undeclared event');
    const subscription: Subscription = { contract, event, listener, ...(onError ? { onError } : {}), queue: [], bytes: 0, running: false, stopped: false }; this.subscriptions.add(subscription);
    return () => { subscription.stopped = true; subscription.queue = []; this.subscriptions.delete(subscription); };
  }
  emit(contract: Contract, event: string, payload: unknown): Promise<void> { return this.host.emitEvent(this.instanceId, contract, event, payload); }
  unload(): Promise<void> { return this.host.unload(this.instanceId); }
  reload(path?: string, options?: { forceWithoutState?: boolean }): Promise<PluginHandle> { return this.host.reload(this.instanceId, path, options); }
}
export class TrustedPluginHost extends EventEmitter {
  readonly limits: Limits; private readonly options: HostOptions; private readonly instances = new Map<string, PluginHandle>(); private readonly serviceRegistry = new PluginRegistry(); private readonly services = new Dispatcher(this.serviceRegistry); private closing = false; private operations = new Set<string>(); private logRedaction = new WeakMap<PluginHandle, { secrets: Buffer[]; pending: Record<'stdout' | 'stderr', Buffer> }>();
  constructor(options: HostOptions = {}) { super(); this.options = options; this.limits = effectiveLimits(options.limits); }
  registerService(contract: Contract, handlers: Record<string, Handler>): this { this.serviceRegistry.add(definePlugin(contract, handlers)); return this; }
  get(instanceId: string): PluginHandle | undefined { return this.instances.get(instanceId); }
  list(): PluginHandle[] { return [...this.instances.values()]; }
  private transition(handle: PluginHandle, status: Status): void { handle.status = status; this.emit('status', { instanceId: handle.instanceId, sessionId: handle.sessionId, status }); }
  assertContract(handle: PluginHandle, contract: Contract): void { const found = handle.resolved.contracts.find(c => c.descriptor.id === contract.descriptor.id); if (!Object.hasOwn(handle.metadata.provides, contract.descriptor.id) || !found || found.digest !== contract.digest || found.descriptor.version !== contract.descriptor.version) throw new PluginError('CONTRACT_MISMATCH', 'Contract not provided by this instance'); }
  private async preflight(path: string, options: LoadOptions): Promise<{ resolved: ResolvedManifest; launch: ResolvedLaunch }> {
    if (options.context !== undefined) object(wireValue(options.context));
    const resolved = await readManifest(path);
    if (!options.development && !resolved.lock) throw new PluginError('INVALID_ARGUMENT', 'Production plugin requires plugin.lock.json. Run napi-vm-plugin pack first, or explicitly use development mode.');
    if (!resolved.contracts.length && this.options.contracts) resolved.contracts = this.options.contracts.filter(c => resolved.manifest.provides[c.descriptor.id]);
    for (const [id, v] of Object.entries(resolved.manifest.provides)) { const c = resolved.contracts.find(c => c.descriptor.id === id); if (!c || c.descriptor.version !== v) throw new PluginError('CONTRACT_MISMATCH', `Missing provided contract artifact ${id}`); const host = this.options.contracts?.find(x => x.descriptor.id === id); if (host && host.digest !== c.digest) throw new PluginError('CONTRACT_MISMATCH', `Configured contract mismatch ${id}`); }
    for (const [id, v] of Object.entries(resolved.manifest.requiresHost)) { const c = this.serviceRegistry.definitions.get(id); const declared = resolved.contracts.find(c => c.descriptor.id === id); if (!c || c.contract.descriptor.version !== v || declared && declared.digest !== c.contract.digest) throw new PluginError('CONTRACT_MISMATCH', `Required host service unavailable: ${id}`); }
    for (const id of resolved.manifest.dependencies?.capabilities ?? []) if (!this.serviceRegistry.definitions.has(id)) throw new PluginError('CONTRACT_MISMATCH', `Required SDK capability unavailable: ${id}`);
    const required = resolved.manifest.dependencies?.services ?? [];
    for (const dep of required) { if (dep.owner === 'host') throw new PluginError('INVALID_ARGUMENT', 'TypeScript host consumes external services only; use Rust for host-owned native service lifecycle'); const supplied = options.services?.[dep.name]; if (!supplied) throw new PluginError('INVALID_ARGUMENT', `Required ${dep.kind} service ${dep.name} must be explicitly injected`); check(boundedString(supplied.url, 8192), 'Injected service URL is invalid'); const url = new URL(supplied.url); check(['http:', 'https:'].includes(url.protocol), 'Injected services require HTTP(S) URL'); if (supplied.headers) { check(Object.keys(supplied.headers).length <= 64, 'Too many service headers'); for (const [key, value] of Object.entries(supplied.headers)) check(/^[!#$%&'*+.^_`|~0-9A-Za-z-]{1,128}$/.test(key) && typeof value === 'string' && value.length <= 8192 && !/[\r\n]/.test(value), 'Invalid injected service header'); } }
    for (const name of Object.keys(options.services ?? {})) check(required.some(d => d.name === name), `Undeclared injected service ${name}`);
    const launch = await resolveLaunch(resolved, { ...options, ...(this.options.runtimePaths ? { runtimePaths: this.options.runtimePaths } : {}) }); return { resolved, launch };
  }
  async load(path: string, options: LoadOptions = {}): Promise<PluginHandle> {
    if (this.closing) throw new PluginError('NOT_READY', 'Host is shutting down'); const { resolved, launch } = await this.preflight(path, options);
    if (this.closing) throw new PluginError('NOT_READY', 'Host is shutting down');
    const handle = new PluginHandle(this, resolved, options); if (this.instances.has(handle.instanceId)) throw new PluginError('INVALID_ARGUMENT', 'Duplicate instanceId'); this.instances.set(handle.instanceId, handle);
    try { await this.start(handle, launch, null); return handle; } catch (error) { this.transition(handle, 'FAILED'); await this.terminate(handle); throw error; }
  }
  private context(handle: PluginHandle, request: RequestContext): CallContext {
    const session = handle.sessionId, peer = handle.peer;
    const current = (): void => { if (handle.sessionId !== session || handle.peer !== peer || !peer || peer.isClosed) throw new PluginError('CONNECTION_CLOSED', 'Call context belongs to a retired session'); };
    return { ...request,
      invoke: async (contract, method, input, options = {}) => { current(); return handle.invoke(contract, method, input, { ...options, context: request }); },
      client(contract) { current(); return createClient(contract, this); },
      emit: async (contract, event, payload) => { current(); return this.emitEvent(handle.instanceId, contract, event, payload, request.id); },
      subscribe: (contract, event, listener, onError) => { current(); return handle.subscribe(contract, event, listener, onError); },
      log: (...values) => this.emit('serviceLog', values),
      spawnTask: () => { throw new PluginError('INVALID_ARGUMENT', 'Host owns service background tasks outside plugin lifecycle'); },
    };
  }
  private async start(handle: PluginHandle, launch: ResolvedLaunch, snapshot: Snapshot | null): Promise<void> {
    if (this.closing) throw new PluginError('NOT_READY', 'Host is shutting down');
    handle.sessionId = randomUUID(); handle.selectedRuntime = launch; handle.lastEvent = 0n; handle.outgoingEvent = 0n; this.transition(handle, 'STARTING');
    const session = handle.sessionId; const token = randomBytes(32).toString('hex');
    const secretValues = [token, ...Object.values(handle.options.env ?? {}), ...Object.values(handle.options.services ?? {}).flatMap(service => Object.values(service.headers ?? {}))];
    for (const service of Object.values(handle.options.services ?? {})) { for (const [name, value] of Object.entries(service.headers ?? {})) if (/^(?:proxy-)?authorization$/i.test(name)) { const credential = /^[^\s]+\s+(.+)$/.exec(value)?.[1]?.trim(); if (credential) secretValues.push(credential); } const url = new URL(service.url); if (url.username) secretValues.push(url.username, decodeURIComponent(url.username)); if (url.password) secretValues.push(url.password, decodeURIComponent(url.password)); for (const value of url.searchParams.values()) if (value) secretValues.push(value, encodeURIComponent(value)); if (url.username || url.password || url.search) secretValues.push(service.url); }
    errorSecrets.set(handle, [...new Set(secretValues.filter(Boolean))].sort((a, b) => b.length - a.length));
    this.logRedaction.set(handle, { secrets: [...new Set(secretValues.filter(Boolean))].map(v => Buffer.from(v)).sort((a, b) => b.length - a.length), pending: { stdout: Buffer.alloc(0), stderr: Buffer.alloc(0) } }); const candidates = new Set<RpcPeer>(); let accepted = false;
    let resolveHello!: (peer: RpcPeer) => void, rejectHello!: (error: unknown) => void;
    const hello = new Promise<RpcPeer>((resolve, reject) => { resolveHello = resolve; rejectHello = reject; });
    const server: Server = createServer((socket: Socket) => {
      if (accepted || candidates.size >= 8) { socket.destroy(); return; }
      const peer = new RpcPeer(socket, { role: 'h', sessionId: session, limits: this.limits, bootstrap: true }); candidates.add(peer);
      const timeout = setTimeout(() => peer.close(), Math.min(2000, this.limits.startupTimeoutMs));
      peer.once('closed', () => { clearTimeout(timeout); candidates.delete(peer); if (accepted && handle.peer === peer && !['STOPPING', 'STOPPED', 'FAILED'].includes(handle.status)) { this.transition(handle, 'FAILED'); void this.terminate(handle); } });
      let greeted = false;
      peer.onRequest('system.hello', params => {
        let authorizedCandidate = false;
        try {
          if (greeted || accepted) throw new PluginError('PROTOCOL_MISMATCH', 'Only one hello is permitted'); greeted = true;
          const p = object(params); exactKeys(p, ['token', 'instanceId', 'sessionId', 'pluginId', 'pluginVersion', 'protocol', 'interfaces', 'requiresHost', 'extensions'], ['token', 'instanceId', 'sessionId', 'pluginId', 'pluginVersion', 'protocol', 'interfaces', 'requiresHost', 'extensions']);
          if (typeof p.token === 'string' && p.token.length === token.length && /^[a-f0-9]{64}$/.test(p.token) && timingSafeEqual(Buffer.from(p.token), Buffer.from(token))) authorizedCandidate = true;
          if (!authorizedCandidate || p.instanceId !== handle.instanceId || p.sessionId !== session || p.pluginId !== handle.metadata.id || p.pluginVersion !== handle.metadata.version) throw new PluginError('PROTOCOL_MISMATCH', 'Handshake identity mismatch');
          const protocol = object(p.protocol); exactKeys(protocol, ['major', 'minMinor', 'maxMinor'], ['major', 'minMinor', 'maxMinor']); if (protocol.major !== 1 || !Number.isSafeInteger(protocol.minMinor) || !Number.isSafeInteger(protocol.maxMinor) || (protocol.minMinor as number) !== 0 || (protocol.maxMinor as number) < 0) throw new PluginError('PROTOCOL_MISMATCH', 'No protocol version overlap');
          check(Array.isArray(p.extensions) && p.extensions.length <= 32 && p.extensions.every(e => typeof e === 'string' && e.length <= 128) && new Set(p.extensions).size === p.extensions.length, 'Invalid extensions');
          matchIdentities(identities(handle.resolved.contracts.filter(c => Object.hasOwn(handle.metadata.provides, c.descriptor.id))), p.interfaces);
          const required = this.serviceRegistry.contracts.filter(c => Object.hasOwn(handle.metadata.requiresHost, c.descriptor.id)); matchIdentities(identities(required), p.requiresHost);
          accepted = true; clearTimeout(timeout); handle.peer = peer; peer.negotiate(this.limits); for (const other of candidates) if (other !== peer) other.close(); server.close();
          peer.onRequest('system.invoke', (params, request) => { if (!['STARTING', 'READY', 'DRAINING'].includes(handle.status)) throw new PluginError('NOT_READY', 'Session is stopping'); const p = object(params); if (typeof p.interface !== 'string' || !Object.hasOwn(handle.metadata.requiresHost, p.interface)) throw new PluginError('CONTRACT_MISMATCH', 'Host service was not negotiated'); return this.services.dispatch(params, this.context(handle, request)); });
          peer.onRequest('system.ping', params => { exactKeys(object(params), []); return null; }); peer.on('event', params => this.receiveEvent(handle, session, params));
          // Resolve only after the hello response has been enqueued; initialize must follow it on the wire.
          setImmediate(() => resolveHello(peer));
          return wireValue({ protocol: { major: 1, minor: 0 }, interfaces: identities(this.serviceRegistry.contracts), limits: this.limits, extensions: [] });
        } catch (error) { if (authorizedCandidate) rejectHello(asPluginError(error)); setTimeout(() => peer.close(), 10); throw error; }
      });
    });
    try {
      await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
      const address = server.address(); if (!address || typeof address === 'string') throw new PluginError('INTERNAL_ERROR', 'Listener unavailable');
      const env = { ...process.env, ...handle.options.env, NAPI_VM_PLUGIN_ENDPOINT: `127.0.0.1:${address.port}`, NAPI_VM_PLUGIN_TOKEN: token, NAPI_VM_PLUGIN_INSTANCE_ID: handle.instanceId, NAPI_VM_PLUGIN_SESSION_ID: session, BUN_CONFIG_NO_CLEAR_TERMINAL: '1' };
      const child = spawn(launch.command, launch.args, { cwd: launch.cwd, env, shell: false, stdio: ['ignore', 'pipe', 'pipe'] }); handle.child = child;
      handle.exitPromise = new Promise<void>(resolve => { child.once('exit', () => resolve()); child.once('error', () => resolve()); });
      child.stdout?.on('data', (chunk: Buffer) => this.log(handle, 'stdout', chunk)); child.stderr?.on('data', (chunk: Buffer) => this.log(handle, 'stderr', chunk));
      child.stdout?.on('end', () => this.log(handle, 'stdout', Buffer.alloc(0), true)); child.stderr?.on('end', () => this.log(handle, 'stderr', Buffer.alloc(0), true));
      child.once('error', error => rejectHello(new PluginError('PLUGIN_EXITED', `Plugin could not start: ${error.message}`)));
      child.once('exit', (code, signal) => { const error = new PluginError('PLUGIN_EXITED', `Plugin exited (${code ?? signal})`); rejectHello(error); handle.peer?.close(error); if (!['STOPPING', 'STOPPED', 'FAILED'].includes(handle.status)) this.transition(handle, 'FAILED'); });
      await bounded((async () => { const peer = await hello; const context: JsonObject = { ...object(wireValue(handle.options.context === undefined ? {} : handle.options.context)) } as JsonObject; if (handle.options.services) context.services = wireValue(handle.options.services); context.capabilities = wireValue(handle.metadata.dependencies?.capabilities ?? []); const native: JsonObject = {}; for (const dep of handle.metadata.dependencies?.native ?? []) native[dep.name] = wireValue({ path: await packagePath(handle.resolved.directory, dep.path), target: dep.target, abi: dep.abi, runtime: dep.runtime }); context.nativeDependencies = native;
        await peer.request('system.initialize', { configuration: handle.options.configuration === undefined ? {} : handle.options.configuration, context, snapshot: wireValue(snapshot) }, { timeoutMs: this.limits.startupTimeoutMs }, initialized => {
          if (initialized !== null) throw new PluginError('INVALID_RESULT', 'Initialize must return null');
          if (peer.isClosed || handle.peer !== peer || handle.sessionId !== session || handle.status !== 'STARTING') throw new PluginError('PLUGIN_EXITED', 'Plugin disconnected during initialization');
          this.transition(handle, 'READY');
        });
      })(), this.limits.startupTimeoutMs, 'Plugin startup/initialization timed out');
      if (handle.sessionId !== session || handle.status !== 'READY' || !handle.peer || handle.peer.isClosed) throw new PluginError('PLUGIN_EXITED', 'Plugin disconnected during initialization');
    } catch (error) { const failure = errorSanitizer(handle)(error); await this.terminate(handle); throw failure; }
    finally { server.close(); for (const peer of candidates) if (peer !== handle.peer || !accepted) peer.close(); }
  }
  private log(handle: PluginHandle, stream: 'stdout' | 'stderr', chunk: Buffer, final = false): void {
    const redaction = this.logRedaction.get(handle) ?? { secrets: [], pending: { stdout: Buffer.alloc(0), stderr: Buffer.alloc(0) } };
    const input = Buffer.concat([redaction.pending[stream], chunk]); const maxSecret = redaction.secrets[0]?.length ?? 1; const safeEnd = final ? input.length : Math.max(0, input.length - maxSecret + 1);
    const output: Buffer[] = []; let cursor = 0, plainStart = 0;
    while (cursor < safeEnd) { const secret = redaction.secrets.find(secret => cursor + secret.length <= input.length && input.subarray(cursor, cursor + secret.length).equals(secret)); if (secret) { if (cursor > plainStart) output.push(input.subarray(plainStart, cursor)); output.push(Buffer.from('[REDACTED]')); cursor += secret.length; plainStart = cursor; } else cursor++; }
    if (cursor > plainStart) output.push(input.subarray(plainStart, cursor)); redaction.pending[stream] = Buffer.from(input.subarray(cursor)); chunk = Buffer.concat(output); if (!chunk.length) return;
    const max = this.limits.maxLogBytes; let retained = chunk;
    if (chunk.length > max) { handle.droppedLogBytes += chunk.length - max; retained = chunk.subarray(chunk.length - max); }
    while (handle.logBytes + retained.length > max && handle.logs.length) { const old = handle.logs.shift()!; const size = Buffer.byteLength(old.text); handle.logBytes -= size; handle.droppedLogBytes += size; }
    let text = retained.toString('utf8'); while (Buffer.byteLength(text) > max) { text = text.slice(1); handle.droppedLogBytes++; } handle.logs.push({ stream, text }); handle.logBytes += Buffer.byteLength(text); this.emit('log', { instanceId: handle.instanceId, stream, text });
  }
  private receiveEvent(handle: PluginHandle, session: string, value: unknown): void {
    try {
      if (handle.sessionId !== session) { handle.discardedStaleEvents++; return; } // Retired generations cannot close or deliver into the replacement.
      if (!['READY', 'DRAINING'].includes(handle.status)) throw new PluginError('NOT_READY', 'Premature business event');
      const e = object(value); exactKeys(e, ['interface', 'version', 'event', 'sessionId', 'sequence', 'payload', 'relatedRequestId'], ['interface', 'version', 'event', 'sessionId', 'sequence', 'payload']);
      if (e.sessionId !== session || typeof e.sequence !== 'string' || !/^[1-9][0-9]{0,30}$/.test(e.sequence) || BigInt(e.sequence) <= handle.lastEvent) throw new PluginError('INVALID_REQUEST', 'Invalid event session/sequence');
      const c = handle.resolved.contracts.find(c => c.descriptor.id === e.interface && c.descriptor.version === e.version); if (!c || typeof e.event !== 'string' || !c.descriptor.events[e.event]) throw new PluginError('CONTRACT_MISMATCH', 'Undeclared event');
      const payload = validate(c, c.descriptor.events[e.event]!.payload, e.payload); const bytes = Buffer.byteLength(JSON.stringify(payload)); handle.lastEvent = BigInt(e.sequence);
      for (const sub of handle.subscriptions) if (!sub.stopped && sub.contract.descriptor.id === e.interface && sub.event === e.event) {
        if (sub.queue.length >= this.limits.maxEventQueue || sub.bytes + bytes > this.limits.maxQueuedBytes) { sub.stopped = true; sub.queue = []; handle.subscriptions.delete(sub); const error = new PluginError('OVERLOADED', 'Event subscriber queue overflow'); try { sub.onError?.(error); } catch {} this.emit('subscriptionError', error); continue; }
        sub.queue.push({ payload, sessionId: session, bytes }); sub.bytes += bytes; if (!sub.running) void this.drainSubscription(handle, sub);
      }
    } catch (error) { handle.peer?.close(asPluginError(error)); }
  }
  private async drainSubscription(handle: PluginHandle, sub: Subscription): Promise<void> {
    sub.running = true; await new Promise<void>(resolve => setImmediate(resolve));
    try { while (!sub.stopped && sub.queue.length) { const item = sub.queue.shift()!; sub.bytes -= item.bytes; if (item.sessionId !== handle.sessionId) { handle.discardedStaleEvents++; continue; } await sub.listener(item.payload); } }
    catch (error) { sub.stopped = true; sub.queue = []; handle.subscriptions.delete(sub); const failure = asPluginError(error); try { sub.onError?.(failure); } catch {} this.emit('subscriptionError', failure); }
    finally { sub.running = false; }
  }
  async emitEvent(instanceId: string, contract: Contract, event: string, payload: unknown, relatedRequestId?: string): Promise<void> {
    const handle = this.instances.get(instanceId);
    if (!handle || handle.status !== 'READY' || !handle.peer) throw new PluginError('NOT_READY', 'Host event requires READY session');
    const service = this.serviceRegistry.definitions.get(contract.descriptor.id);
    if (!service || service.contract.digest !== contract.digest || !Object.hasOwn(handle.metadata.requiresHost, contract.descriptor.id)) throw new PluginError('CONTRACT_MISMATCH', 'Host event interface was not negotiated');
    const type = Object.hasOwn(contract.descriptor.events, event) ? contract.descriptor.events[event] : undefined; if (!type) throw new PluginError('INVALID_ARGUMENT', 'Undeclared host event');
    handle.peer.notify('system.event', { interface: contract.descriptor.id, version: contract.descriptor.version, event, sessionId: handle.sessionId, sequence: String(++handle.outgoingEvent), payload: validate(contract, type.payload, payload), ...(relatedRequestId ? { relatedRequestId } : {}) });
  }
  private async terminate(handle: PluginHandle): Promise<void> {
    handle.peer?.close(); handle.peer = undefined; const child = handle.child;
    if (child && child.exitCode === null && child.signalCode === null) {
      child.kill('SIGTERM');
      try { await bounded(handle.exitPromise ?? Promise.resolve(), Math.min(this.limits.shutdownTimeoutMs, 1000), 'Termination timed out'); }
      catch { child.kill('SIGKILL'); await handle.exitPromise; }
    }
    if (handle.exitPromise) await handle.exitPromise;
    if (child) {
      const drained = () => (!child.stdout || child.stdout.destroyed || child.stdout.readableEnded) && (!child.stderr || child.stderr.destroyed || child.stderr.readableEnded);
      if (!drained()) await bounded(new Promise<void>(resolve => child.once('close', resolve)), 100, 'Log drain deadline').catch(() => {});
      child.stdout?.destroy(); child.stderr?.destroy(); this.log(handle, 'stdout', Buffer.alloc(0), true); this.log(handle, 'stderr', Buffer.alloc(0), true);
    }
    handle.child = undefined; this.logRedaction.delete(handle);
  }
  private async stop(handle: PluginHandle): Promise<void> {
    this.transition(handle, 'STOPPING');
    try { if (handle.peer && !handle.peer.isClosed) await handle.peer.request('system.shutdown', {}, { timeoutMs: this.limits.shutdownTimeoutMs }); }
    catch { /* a failed cleanup cannot keep an owned process alive */ }
    finally { await this.terminate(handle); this.transition(handle, 'STOPPED'); }
  }
  async unload(instanceId: string): Promise<void> {
    const handle = this.instances.get(instanceId); if (!handle || handle.status === 'STOPPED') return;
    if (this.operations.has(instanceId)) throw new PluginError('NOT_READY', 'Lifecycle operation already in progress'); this.operations.add(instanceId);
    try { this.transition(handle, 'DRAINING'); try { await handle.peer?.request('system.quiesce', { timeoutMs: this.limits.shutdownTimeoutMs }, { timeoutMs: this.limits.shutdownTimeoutMs }); } catch { handle.peer?.cancelActive(); } await this.stop(handle); for (const sub of handle.subscriptions) { sub.stopped = true; sub.queue = []; } handle.subscriptions.clear(); }
    finally { this.operations.delete(instanceId); this.emit('operationDone', instanceId); }
  }
  async reload(instanceId: string, replacementPath?: string, options: { forceWithoutState?: boolean } = {}): Promise<PluginHandle> {
    const handle = this.instances.get(instanceId); if (!handle) throw new PluginError('INVALID_ARGUMENT', 'Unknown instance'); if (this.operations.has(instanceId)) throw new PluginError('NOT_READY', 'Lifecycle operation already in progress'); this.operations.add(instanceId);
    try {
      const replacement = await this.preflight(replacementPath ?? handle.resolved.path, handle.options);
      matchIdentities(identities(handle.resolved.contracts.filter(c => Object.hasOwn(handle.metadata.provides, c.descriptor.id))), identities(replacement.resolved.contracts.filter(c => Object.hasOwn(replacement.resolved.manifest.provides, c.descriptor.id))));
      let snapshot = options.forceWithoutState ? null : handle.snapshot ?? null;
      if (handle.status !== 'FAILED' && handle.status !== 'STOPPED') {
        this.transition(handle, 'DRAINING');
        if (!options.forceWithoutState) {
          if (!handle.peer) throw new PluginError('CONNECTION_CLOSED', 'Missing session during reload');
          const quiesced = await handle.peer.request('system.quiesce', { timeoutMs: this.limits.shutdownTimeoutMs }, { timeoutMs: this.limits.shutdownTimeoutMs });
          if (quiesced !== null) throw new PluginError('INVALID_RESULT', 'Quiesce must return null');
          const raw = await handle.peer.request('system.snapshot', {}, { timeoutMs: this.limits.shutdownTimeoutMs });
          snapshot = raw === null && !handle.resolved.contracts.some(c => c.descriptor.state) ? null : validateSnapshot(handle.resolved.contracts, raw); handle.snapshot = snapshot;
        }
        await this.stop(handle);
      }
      if (snapshot) validateSnapshot(replacement.resolved.contracts, snapshot);
      handle.resolved = replacement.resolved; handle.metadata = replacement.resolved.manifest;
      for (const sub of handle.subscriptions) { handle.discardedStaleEvents += sub.queue.length; sub.queue = []; sub.bytes = 0; }
      try { await this.start(handle, replacement.launch, snapshot); handle.snapshot = undefined; return handle; }
      catch (error) { this.transition(handle, 'FAILED'); handle.snapshot = snapshot; await this.terminate(handle); throw error; }
    } catch (error) { throw errorSanitizer(handle)(error); } finally { this.operations.delete(instanceId); this.emit('operationDone', instanceId); }
  }
  async shutdown(): Promise<void> {
    this.closing = true;
    await Promise.all(this.list().map(async h => {
      if (this.operations.has(h.instanceId)) await bounded(new Promise<void>(resolve => { const done = (id: string) => { if (id === h.instanceId) { this.off('operationDone', done); resolve(); } }; this.on('operationDone', done); }), this.limits.shutdownTimeoutMs, 'Lifecycle operation exceeded shutdown deadline').catch(async () => { await this.terminate(h); });
      if (this.operations.has(h.instanceId)) { await this.terminate(h); this.transition(h, 'STOPPED'); } else await this.unload(h.instanceId);
    }));
  }
}
export { TrustedPluginHost as PluginHost };
