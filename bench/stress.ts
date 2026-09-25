/**
 * Stress tests replicating the VmScriptsPlugin + EventBusPlugin architecture.
 *
 * Targets the exact memory-leak vectors in the plugin system:
 *   1. Hot-reload cycles (teardown + rebuild) leaking stale VM/bus references
 *   2. Listener accumulation when unsubscribe is missed
 *   3. runAsync fire-and-forget holding references to torn-down VMs
 *   4. Middleware chain growth under sustained emit pressure
 *   5. Large JSON payloads marshalled across the NAPI boundary repeatedly
 *   6. Pattern-matching filter overhead with hundreds of handlers
 *
 * Run:  bun bench/stress.ts
 *       bun bench/stress.ts --quick   (reduced iterations for CI)
 */

import { Vm } from "../index";

// ── Config ───────────────────────────────────────────────────────────

const QUICK = process.argv.includes("--quick");

const CFG = {
  /** Hot-reload cycles (VM teardown + rebuild). */
  reloadCycles: QUICK ? 10 : 100,
  /** Listeners registered per reload cycle. */
  listenersPerCycle: QUICK ? 10 : 50,
  /** Emits per cycle while listeners are active. */
  emitsPerCycle: QUICK ? 50 : 500,
  /** Middleware chain depth. */
  middlewareDepth: QUICK ? 5 : 20,
  /** Large JSON payload size (KB). */
  payloadKb: QUICK ? 64 : 512,
  /** runAsync dispatches per cycle. */
  asyncDispatches: QUICK ? 10 : 100,
  /** Pattern-matching handlers for filter stress. */
  filterHandlers: QUICK ? 50 : 500,
  /** Concurrent VMs emitting through a shared bus. */
  concurrentVms: QUICK ? 3 : 8,
};

// ── Minimal EventBus (mirrors the plugin's interface) ────────────────

type Platform = string;
type EventName = string;

interface RawEvent {
  platform: Platform;
  eventName: EventName;
  data: Record<string, unknown>;
}

type Filter = (event: RawEvent) => boolean;
type Handler = (event: RawEvent) => void | Promise<void>;
type Middleware = (event: RawEvent, next: () => void) => void;

interface MiddlewareEntry { filter: Filter; mw: Middleware }
interface HandlerEntry { filter: Filter; handler: Handler; once: boolean }

class EventBus {
  private middlewareEntries: MiddlewareEntry[] = [];
  private handlers: HandlerEntry[] = [];

  use(mw: Middleware): () => void {
    const entry: MiddlewareEntry = { filter: () => true, mw };
    this.middlewareEntries.push(entry);
    return () => {
      const idx = this.middlewareEntries.indexOf(entry);
      if (idx !== -1) this.middlewareEntries.splice(idx, 1);
    };
  }

  useFiltered(filter: Filter, mw: Middleware): () => void {
    const entry: MiddlewareEntry = { filter, mw };
    this.middlewareEntries.push(entry);
    return () => {
      const idx = this.middlewareEntries.indexOf(entry);
      if (idx !== -1) this.middlewareEntries.splice(idx, 1);
    };
  }

  on(filter: Filter, handler: Handler): () => void {
    const entry: HandlerEntry = { filter, handler, once: false };
    this.handlers.push(entry);
    return () => {
      const idx = this.handlers.indexOf(entry);
      if (idx !== -1) this.handlers.splice(idx, 1);
    };
  }

  once(filter: Filter, handler: Handler): () => void {
    const entry: HandlerEntry = { filter, handler, once: true };
    this.handlers.push(entry);
    return () => {
      const idx = this.handlers.indexOf(entry);
      if (idx !== -1) this.handlers.splice(idx, 1);
    };
  }

  async emit(platform: Platform, eventName: EventName, data: Record<string, unknown>): Promise<void> {
    const event: RawEvent = { platform, eventName, data };

    // Run middleware chain
    const matching = this.middlewareEntries.filter((e) => e.filter(event));
    let idx = 0;
    let cancelled = false;
    const runNext = (): void => {
      if (cancelled || idx >= matching.length) return;
      matching[idx++].mw(event, runNext);
    };
    runNext();
    if (cancelled) return;

    // Dispatch to handlers
    const toRemove: HandlerEntry[] = [];
    for (const entry of this.handlers) {
      if (entry.filter(event)) {
        await entry.handler(event);
        if (entry.once) toRemove.push(entry);
      }
    }
    for (const entry of toRemove) {
      const i = this.handlers.indexOf(entry);
      if (i !== -1) this.handlers.splice(i, 1);
    }
  }

  listenerCount(): number { return this.handlers.length; }
  middlewareCount(): number { return this.middlewareEntries.length; }
}

// ── VmScripts-like wrapper (mirrors the plugin's lifecycle) ──────────

interface ListenerEntry { pattern: string; handlerName: string }

class VmScriptsHarness {
  private vm: Vm | null = null;
  private bus: EventBus;
  private listeners: ListenerEntry[] = [];
  private unsubscribers: Array<() => void> = [];
  private alive = false;
  /** Use sync vm.run() for dispatch (safe for immediate teardown). */
  private syncDispatch: boolean;
  /** In-flight runAsync promises (for drain before teardown). */
  private pending: Set<Promise<unknown>> = new Set();

  constructor(bus: EventBus, opts?: { syncDispatch?: boolean }) {
    this.bus = bus;
    this.syncDispatch = opts?.syncDispatch ?? false;
  }

  /** Wait for all in-flight runAsync dispatches to settle. */
  async drain(): Promise<void> {
    while (this.pending.size > 0) {
      await Promise.allSettled([...this.pending]);
    }
  }

  build(): void {
    const vm = new Vm();
    vm.setLoopLimit(10_000_000);
    this.alive = true;

    // Expose bridge (mirrors exposeBridge)
    const bus = this.bus;
    vm.exposeFunction("emit", (platform: string, eventName: string, data: unknown) => {
      const payload = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
      void bus.emit(platform, eventName, payload);
    });

    vm.exposeFunction("on", (pattern: string, handlerName: string) => {
      this.listeners.push({ pattern, handlerName });
      if (this.vm) this.subscribeOne({ pattern, handlerName });
    });

    vm.exposeFunction("log", (..._args: unknown[]) => {});

    this.vm = vm;
    this.subscribeListeners();
  }

  /** Simulate loading a script module that registers handlers. */
  loadScript(name: string, code: string): void {
    if (!this.vm) return;
    this.vm.registerModule(name, code + '\n"";');
  }

  teardown(): void {
    // Mark dead FIRST so in-flight async handlers bail out
    this.alive = false;

    for (const unsub of this.unsubscribers) unsub();
    this.unsubscribers = [];
    this.listeners = [];

    if (this.vm) {
      for (const name of this.vm.listModules()) {
        this.vm.removeModule(name);
      }
      this.vm.removeGlobal("emit");
      this.vm.removeGlobal("on");
      this.vm.removeGlobal("log");
      this.vm = null;
    }
  }

  reload(): void {
    this.teardown();
    this.build();
  }

  getVm(): Vm | null { return this.vm; }

  private subscribeListeners(): void {
    if (!this.vm) return;
    for (const entry of this.listeners) {
      this.subscribeOne(entry);
    }
  }

  private subscribeOne(entry: ListenerEntry): void {
    const filter = this.patternToFilter(entry.pattern);
    const handlerName = entry.handlerName;

    const unsub = this.bus.on(filter, (event: RawEvent) => {
      if (!this.alive || !this.vm) return;
      const vm = this.vm;
      const json = JSON.stringify({ platform: event.platform, eventName: event.eventName, data: event.data });

      if (this.syncDispatch) {
        // Synchronous: safe for immediate teardown after emit resolves.
        try { vm.run(`${handlerName}(${json});`); } catch { /* handler error */ }
      } else {
        // Async: mirrors the real plugin's `void vm.runAsync(...)` pattern.
        // `runAsync` can also throw synchronously (e.g. the VM is busy with
        // another execution); a fire-and-forget dispatch must not let that
        // escape into the emitter.
        try {
          const p = (vm.runAsync(`${handlerName}(${json});`) as Promise<unknown>)
            .catch(() => {})
            .finally(() => { this.pending.delete(p); });
          this.pending.add(p);
        } catch { /* dispatch contention; in-flight work still drains below */ }
      }
    });

    this.unsubscribers.push(unsub);
  }

  private patternToFilter(pattern: string): Filter {
    if (pattern === "*") return () => true;
    if (pattern.includes(":")) {
      const [platform, eventName] = pattern.split(":", 2);
      if (platform === "*") return (e) => e.eventName === eventName;
      if (eventName === "*") return (e) => e.platform === platform;
      return (e) => e.platform === platform && e.eventName === eventName;
    }
    return (e) => e.eventName === pattern;
  }
}

// ── Helpers ──────────────────────────────────────────────────────────

interface StressResult {
  name: string;
  passed: boolean;
  durationMs: number;
  detail: string;
}

const results: StressResult[] = [];

function record(name: string, passed: boolean, durationMs: number, detail: string) {
  results.push({ name, passed, durationMs, detail });
  const icon = passed ? "✓" : "✗";
  console.log(`  ${icon} ${name} — ${detail} (${durationMs.toFixed(0)} ms)`);
}

function memMb(): number {
  return process.memoryUsage().heapUsed / (1024 * 1024);
}

function largeJsonCode(approxKb: number): string {
  const records = Math.ceil((approxKb * 1024) / 120);
  return `(function() {
    var items = [];
    for (var i = 0; i < ${records}; i++) {
      items.push({ id: i, name: "rec_" + i, val: i * 3.14, tags: ["a","b","c"], meta: { x: i, y: i * 2 } });
    }
    return JSON.stringify({ count: ${records}, items: items });
  })()`;
}

// ── Test 1: Hot-reload cycles (the primary leak vector) ─────────────
// Repeatedly teardown + rebuild the VM while the bus stays alive.
// If unsubscribers leak or the old VM is retained, heap grows linearly.

async function testHotReloadCycles() {
  const t0 = performance.now();
  const bus = new EventBus();
  const memBefore = memMb();

  for (let cycle = 0; cycle < CFG.reloadCycles; cycle++) {
    // syncDispatch: handlers complete before emit() resolves, safe for immediate teardown
    const harness = new VmScriptsHarness(bus, { syncDispatch: true });
    harness.build();

    // Register listeners (simulates script top-level `on(...)` calls)
    const vm = harness.getVm()!;
    for (let l = 0; l < CFG.listenersPerCycle; l++) {
      vm.run(`on("platform:event${l % 5}", "handler${l}");`);
      vm.run(`function handler${l}(ev) { return ev; }`);
    }

    // Emit events — sync dispatch means all VM work is done when emit resolves
    for (let e = 0; e < CFG.emitsPerCycle; e++) {
      await bus.emit("platform", `event${e % 5}`, { i: e, cycle });
    }

    harness.teardown();
  }

  if (typeof globalThis.gc === "function") globalThis.gc();
  const growth = memMb() - memBefore;
  const leaked = bus.listenerCount() > 0;

  record(
    "hot-reload-cycles",
    !leaked && growth < 30,
    performance.now() - t0,
    `${CFG.reloadCycles} cycles, bus listeners after: ${bus.listenerCount()}, heap Δ ${growth.toFixed(1)} MB`
  );
}

// ── Test 2: Listener accumulation (missed unsubscribe) ──────────────
// Simulates the bug where `on()` is called mid-session but teardown
// doesn't clean the new subscriptions. Heap + dispatch time should grow.

function testListenerAccumulation() {
  const t0 = performance.now();
  const bus = new EventBus();
  const harness = new VmScriptsHarness(bus, { syncDispatch: true });
  harness.build();

  const vm = harness.getVm()!;
  const totalListeners = CFG.listenersPerCycle * (QUICK ? 5 : 20);

  // Accumulate listeners without teardown (the leak scenario)
  for (let i = 0; i < totalListeners; i++) {
    vm.run(`on("*", "noop");`);
    vm.run(`function noop(ev) { return ev; }`);
  }

  const countAfter = bus.listenerCount();

  // Now emit once — all listeners fire
  const t1 = performance.now();
  void bus.emit("test", "ping", { x: 1 });
  const dispatchMs = performance.now() - t1;

  // Proper teardown should clean everything
  harness.teardown();
  const countAfterTeardown = bus.listenerCount();

  const passed = countAfter === totalListeners && countAfterTeardown === 0;
  record(
    "listener-accumulation",
    passed,
    performance.now() - t0,
    `${totalListeners} accumulated, dispatch: ${dispatchMs.toFixed(1)} ms, after teardown: ${countAfterTeardown}`
  );
}

// ── Test 3: runAsync fire-and-forget under load ─────────────────────
// The plugin uses `void vm.runAsync(...)` — if the VM is torn down while
// async ops are in flight, dangling promises may retain the VM.

async function testAsyncDispatchLoad() {
  const t0 = performance.now();
  const bus = new EventBus();
  const harness = new VmScriptsHarness(bus);
  harness.build();

  const vm = harness.getVm()!;
  vm.run(`function handleChat(ev) { return ev.data.msg; }`);
  vm.run(`on("tiktok:chat", "handleChat");`);

  // Fire many events that trigger runAsync dispatches
  const promises: Promise<void>[] = [];
  for (let i = 0; i < CFG.asyncDispatches; i++) {
    promises.push(
      bus.emit("tiktok", "chat", { msg: `message_${i}`, idx: i, payload: "x".repeat(200) })
    );
  }
  await Promise.all(promises);

  // Drain all in-flight runAsync operations before teardown
  await harness.drain();

  const memBefore = memMb();
  harness.teardown();
  if (typeof globalThis.gc === "function") globalThis.gc();
  const growth = memMb() - memBefore;

  record(
    "async-dispatch-load",
    growth < 20,
    performance.now() - t0,
    `${CFG.asyncDispatches} runAsync dispatches, post-teardown heap Δ ${growth.toFixed(1)} MB`
  );
}

// ── Test 4: Middleware chain depth under sustained pressure ──────────
// Each emit filters ALL middleware. With deep chains + high emit rate,
// this tests whether the filter array is properly cleaned on unsubscribe.

function testMiddlewareChainPressure() {
  const t0 = performance.now();
  const bus = new EventBus();

  const unsubs: Array<() => void> = [];
  let mwExecutions = 0;

  // Build a deep middleware chain
  for (let i = 0; i < CFG.middlewareDepth; i++) {
    unsubs.push(bus.use((_event, next) => {
      mwExecutions++;
      next();
    }));
  }

  // Sustained emits through the chain
  const emits = CFG.emitsPerCycle * (QUICK ? 2 : 10);
  for (let i = 0; i < emits; i++) {
    void bus.emit("platform", "tick", { i });
  }

  const expectedExecs = emits * CFG.middlewareDepth;
  const chainOk = mwExecutions === expectedExecs;

  // Remove half the middleware, emit again
  for (let i = 0; i < unsubs.length; i += 2) unsubs[i]();
  const remaining = bus.middlewareCount();

  for (let i = 0; i < emits; i++) {
    void bus.emit("platform", "tick", { i });
  }

  const expectedAfter = expectedExecs + emits * remaining;
  const cleanupOk = mwExecutions === expectedAfter;

  // Clean the rest
  for (const unsub of unsubs) unsub();

  record(
    "middleware-chain-pressure",
    chainOk && cleanupOk && bus.middlewareCount() === 0,
    performance.now() - t0,
    `${CFG.middlewareDepth} deep × ${emits} emits, ${mwExecutions} executions, remaining: ${bus.middlewareCount()}`
  );
}

// ── Test 5: Large JSON through emit → VM → emit roundtrip ───────────
// Mirrors the plugin receiving a large event, passing it into the VM via
// runAsync(JSON), the VM processing it, and emitting back.

async function testLargeJsonRoundtrip() {
  const t0 = performance.now();
  const bus = new EventBus();
  // Sync dispatch: the VM handler runs inline, so emit("result",...) fires
  // synchronously within the bus handler — no async settle needed.
  const harness = new VmScriptsHarness(bus, { syncDispatch: true });
  harness.build();

  const vm = harness.getVm()!;

  // VM script that processes incoming data and emits a transformed result
  vm.run(`
    function processData(ev) {
      var items = ev.data.items;
      var sum = 0;
      for (var i = 0; i < items.length; i++) { sum += items[i].val; }
      emit("result", "processed", { sum: sum, count: items.length });
      return sum;
    }
  `);
  vm.run(`on("feed:batch", "processData");`);

  let resultReceived = false;
  let resultSum = 0;
  bus.on((e) => e.platform === "result" && e.eventName === "processed", (e) => {
    resultReceived = true;
    resultSum = e.data.sum as number;
  });

  // Generate a large payload and push it through
  const records = Math.ceil((CFG.payloadKb * 1024) / 80);
  const items = Array.from({ length: records }, (_, i) => ({ id: i, val: i * 2 }));

  const iterations = QUICK ? 5 : 30;
  for (let i = 0; i < iterations; i++) {
    await bus.emit("feed", "batch", { items, batch: i });
  }

  harness.teardown();

  const expectedSum = records * (records - 1); // sum of i*2 for i in 0..n-1
  const passed = resultReceived && resultSum === expectedSum;
  record(
    "large-json-roundtrip",
    passed,
    performance.now() - t0,
    `${CFG.payloadKb} KB × ${iterations} iterations, sum=${resultSum} (expected ${expectedSum})`
  );
}

// ── Test 6: Pattern-matching filter stress ───────────────────────────
// Hundreds of handlers with different patterns, every emit checks all.

function testFilterStress() {
  const t0 = performance.now();
  const bus = new EventBus();
  const unsubs: Array<() => void> = [];

  let totalDispatches = 0;
  const platforms = ["tiktok", "twitch", "youtube", "kick", "discord"];
  const events = ["chat", "follow", "sub", "donation", "raid", "gift"];

  // Register many handlers with varied patterns
  for (let i = 0; i < CFG.filterHandlers; i++) {
    const p = platforms[i % platforms.length];
    const e = events[i % events.length];
    const pattern = i % 3 === 0 ? "*" : i % 3 === 1 ? `${p}:*` : `${p}:${e}`;

    const filter = patternToFilter(pattern);
    unsubs.push(bus.on(filter, () => { totalDispatches++; }));
  }

  // Emit across all platform/event combos
  const emitRounds = QUICK ? 10 : 50;
  for (let r = 0; r < emitRounds; r++) {
    for (const p of platforms) {
      for (const e of events) {
        void bus.emit(p, e, { round: r });
      }
    }
  }

  // Cleanup
  for (const unsub of unsubs) unsub();
  const cleaned = bus.listenerCount() === 0;

  record(
    "filter-stress",
    cleaned && totalDispatches > 0,
    performance.now() - t0,
    `${CFG.filterHandlers} handlers × ${emitRounds * platforms.length * events.length} emits, ${totalDispatches} dispatches`
  );
}

function patternToFilter(pattern: string): Filter {
  if (pattern === "*") return () => true;
  if (pattern.includes(":")) {
    const [platform, eventName] = pattern.split(":", 2);
    if (platform === "*") return (e) => e.eventName === eventName;
    if (eventName === "*") return (e) => e.platform === platform;
    return (e) => e.platform === platform && e.eventName === eventName;
  }
  return (e) => e.eventName === pattern;
}

// ── Test 7: Concurrent VMs on shared bus (multi-plugin scenario) ────

async function testConcurrentVmsSharedBus() {
  const t0 = performance.now();
  const bus = new EventBus();
  const harnesses: VmScriptsHarness[] = [];

  for (let v = 0; v < CFG.concurrentVms; v++) {
    const h = new VmScriptsHarness(bus, { syncDispatch: true });
    h.build();
    const vm = h.getVm()!;
    vm.run(`function handler${v}(ev) { return ev.data.n; }`);
    vm.run(`on("shared:data", "handler${v}");`);
    harnesses.push(h);
  }

  // All VMs listen on the same event — each emit triggers all of them
  const emits = CFG.emitsPerCycle;
  for (let i = 0; i < emits; i++) {
    await bus.emit("shared", "data", { n: i });
  }

  // Let async dispatches settle
  await new Promise((r) => setTimeout(r, 100));

  // Teardown all — bus should be empty
  for (const h of harnesses) h.teardown();
  const clean = bus.listenerCount() === 0;

  record(
    "concurrent-vms-shared-bus",
    clean,
    performance.now() - t0,
    `${CFG.concurrentVms} VMs × ${emits} emits, listeners after teardown: ${bus.listenerCount()}`
  );
}

// ── Test 8: Memory stability over sustained mixed workload ───────────

async function testMemoryStability() {
  const t0 = performance.now();
  const bus = new EventBus();

  // Add middleware that stays for the whole test
  const mwUnsubs: Array<() => void> = [];
  for (let i = 0; i < 5; i++) {
    mwUnsubs.push(bus.use((_e, next) => next()));
  }

  const memBefore = memMb();
  const cycles = QUICK ? 10 : 50;

  for (let c = 0; c < cycles; c++) {
    const harness = new VmScriptsHarness(bus, { syncDispatch: true });
    harness.build();
    const vm = harness.getVm()!;

    // Load a "script" that registers handlers and emits
    vm.run(`function onChat(ev) { emit("out", "log", { msg: ev.data.msg }); }`);
    vm.run(`on("in:chat", "onChat");`);

    // Push events through
    for (let i = 0; i < 20; i++) {
      await bus.emit("in", "chat", { msg: `m${c}_${i}`, blob: "x".repeat(500) });
    }

    harness.teardown();
  }

  for (const unsub of mwUnsubs) unsub();
  if (typeof globalThis.gc === "function") globalThis.gc();

  const growth = memMb() - memBefore;
  const passed = growth < 30 && bus.listenerCount() === 0;

  record(
    "memory-stability",
    passed,
    performance.now() - t0,
    `${cycles} mixed cycles, heap Δ ${growth.toFixed(1)} MB, listeners: ${bus.listenerCount()}`
  );
}

// ── Main ─────────────────────────────────────────────────────────────

async function main() {
  const engine = typeof Bun !== "undefined" ? "Bun" : `Node ${process.version}`;
  console.log(`\nnapi-vm plugin-architecture stress tests (${engine}, ${process.platform}/${process.arch})`);
  console.log(`Mode: ${QUICK ? "quick (CI)" : "full"}\n`);
  console.log(`Config: ${CFG.reloadCycles} reloads, ${CFG.listenersPerCycle} listeners/cycle, ` +
    `${CFG.emitsPerCycle} emits/cycle, ${CFG.payloadKb} KB payloads, ` +
    `${CFG.middlewareDepth} middleware depth\n`);

  const memStart = memMb();

  await testHotReloadCycles();
  testListenerAccumulation();
  await testAsyncDispatchLoad();
  testMiddlewareChainPressure();
  await testLargeJsonRoundtrip();
  testFilterStress();
  await testConcurrentVmsSharedBus();
  await testMemoryStability();

  const memEnd = memMb();
  const failed = results.filter((r) => !r.passed);

  console.log(`\n${"─".repeat(64)}`);
  console.log(`Results: ${results.length - failed.length}/${results.length} passed`);
  console.log(`Heap: ${memStart.toFixed(1)} MB → ${memEnd.toFixed(1)} MB (Δ ${(memEnd - memStart).toFixed(1)} MB)`);

  if (failed.length > 0) {
    console.log(`\nFailed:`);
    for (const f of failed) {
      console.log(`  ✗ ${f.name}: ${f.detail}`);
    }
    process.exit(1);
  }

  console.log("\nAll stress tests passed.");
}

main();
