/**
 * napi-vm crash-safety harness — every known way guest code can kill the host.
 *
 * Run:   bun examples/crash.ts            (full matrix, one subprocess per case)
 *        bun examples/crash.ts --case deep-recursion   (single case, in-process)
 *        bun examples/crash.ts --only oom-array,shared-result-formatting (isolated cases)
 *
 * Why subprocesses: historically several cases below were *genuine process
 * killers* — native Rust stack overflows (SIGSEGV) and memory exhaustion
 * (SIGTRAP/SIGABRT). A native stack overflow cannot be caught by `try/catch`
 * in the VM, by `process.on("uncaughtException")` in Node, or by napi-rs's
 * `catch_unwind`: it is a signal, not a JS or Rust exception. The interpreter
 * is now hardened so every case is contained in-process — but the harness
 * still runs each case in a disposable child, because that is the only way to
 * *prove* containment: if a guard regresses, the child dies and the verdict
 * says so, instead of taking this harness down with it.
 *
 * Children run under **node**, not bun, deliberately: bun installs its own
 * SIGSEGV handler (for JavaScriptCore), which intercepts the VM's stack
 * overflows and turns a clean crash into an indefinite hang — masking the
 * real behavior. Node has no such handler, so the signal lands as-is, which
 * is also what a production consumer of this CJS binding would see.
 *
 * For every case the parent records a verdict:
 *
 *   SURVIVED  child exited 0 — the VM handled the code and returned
 *   THROWN    child exited 0 via a catchable JS error — also correct
 *             containment (this is what V8 does for stack overflow)
 *   CRASHED   child died on a signal (SIGSEGV / SIGTRAP / SIGABRT) — the
 *             guest code took down the whole host process
 *   HANG      child had to be killed after the timeout — guest code froze
 *             the host event loop forever
 *
 * Each case's `expected` field pins the contained behavior, so regressions
 * are visible: the interpreter now has a call-depth limit, a parse-depth
 * limit, a loop budget, cycle detection, allocation size caps, and an
 * iterative Drop, so every case must end SURVIVED or THROWN. Any CRASHED/HANG
 * row — or any verdict that disagrees with `expected` — fails the harness
 * (exit code 1), which makes this file a CI gate for crash safety.
 */

import { spawn } from "node:child_process";

// ── Case definitions ─────────────────────────────────────────────────

interface CaseOpts {
  /** Per-case timeout in ms before the child is killed and verdict = HANG. */
  timeoutMs?: number;
  /** Linux virtual-memory cap (KB) applied via `ulimit -v`, for OOM cases. */
  memLimitKb?: number;
}

interface CrashCase {
  id: string;
  title: string;
  /** The adversarial guest code (run inside `vm.run` in the child). */
  code: string;
  /** Execute the case through the worker-thread `runAsync` bridge. */
  asyncRun?: boolean;
  opts?: CaseOpts;
  expected: "SURVIVED" | "THROWN" | "CRASHED" | "HANG";
  note: string;
}

const CASES: CrashCase[] = [
  // ── Native stack overflow: the big one ─────────────────────────────
  // The interpreter is tree-walking: each VM call frame is a real Rust
  // frame (eval_stmt → call_this → eval_stmt → …). A call-depth counter
  // (MAX_CALL_DEPTH = 256) stops unbounded guest recursion long before
  // the native stack is in danger, raising a catchable RangeError —
  // exactly what V8's "Maximum call stack size exceeded" does.
  {
    id: "deep-recursion",
    title: "Unbounded direct recursion",
    code: "function f() { return f(); } f();",
    expected: "THROWN",
    note: "RangeError: Maximum call stack size exceeded (catchable)",
  },
  {
    id: "mutual-recursion",
    title: "Unbounded mutual recursion",
    code: "function a() { return b(); } function b() { return a(); } a();",
    expected: "THROWN",
    note: "same call-depth guard, two functions",
  },
  {
    id: "async-worker-recursion",
    title: "Recursion on the runAsync worker",
    code: "async function main() { function f(n) { return n ? f(n - 1) : 0; } return f(300); } main();",
    asyncRun: true,
    expected: "THROWN",
    note: "the worker uses the same call-depth guard with an 8MB stack",
  },
  {
    id: "deep-parse",
    title: "100k-deep nested parentheses",
    code: "(".repeat(100_000) + "1" + ")".repeat(100_000),
    expected: "THROWN",
    note: "parser nesting cap (MAX_PARSE_DEPTH) rejects it before execution",
  },

  // ── Cyclic structures: serialization is cycle- and depth-bounded ───
  // to_string (vm.run's return value, console.log, getGlobal) walks
  // objects with a visited set and prints "[Circular]"; JSON.stringify
  // detects cycles and throws a catchable TypeError. Both have depth
  // caps — the same containment real JS engines provide.
  {
    id: "cyclic-object-return",
    title: "Return a cyclic object",
    code: "let o = {}; o.self = o; o;",
    expected: "SURVIVED",
    note: "to_string prints {self: [Circular]}",
  },
  {
    id: "cyclic-array-return",
    title: "Return a cyclic array",
    code: "let a = []; a.push(a); a;",
    expected: "SURVIVED",
    note: "to_string prints [[Circular]]",
  },
  {
    id: "cyclic-json-stringify",
    title: "JSON.stringify a cyclic object",
    code: "let o = {}; o.self = o; JSON.stringify(o);",
    expected: "THROWN",
    note: "TypeError: Converting circular structure to JSON (catchable)",
  },

  // ── Recursive Drop: used to crash *after* the code succeeded ───────
  // A million-deep [ [ [ 0 ] ] ] builds fine — and now tears down fine:
  // Value's Drop is iterative (an explicit work stack), so freeing the
  // structure costs O(1) native stack, not O(depth).
  {
    id: "deep-nesting-drop",
    title: "1M-deep nesting, safe teardown",
    code: "let a = [0]; for (let i = 0; i < 1000000; i++) { a = [a]; } 'built';",
    expected: "SURVIVED",
    note: "iterative Drop survives teardown at any depth",
  },

  // ── Resource exhaustion ────────────────────────────────────────────
  // Every loop iteration consumes from a budget (default 100M, tunable
  // via vm.setLoopLimit, refilled on each vm.run), so infinite loops
  // throw a catchable RangeError instead of blocking the event loop.
  // Arrays have a dense-growth threshold and operation-specific materialization
  // limits, not a universal logical-length or aggregate heap cap. Strings have
  // bounded construction paths (64 Mi UTF-16 units) and result formatting has
  // a 64 MiB output budget. The `ulimit -v 2GB` wrapper is a backstop if a guard
  // regresses, not a total guest heap guarantee. Node v26 reserves ~1.5GB of
  // virtual address space at startup, so this wrapper cannot go lower.
  {
    id: "infinite-loop",
    title: "while (true) {} — loop budget",
    code: "while (true) {}",
    opts: { timeoutMs: 15_000 },
    expected: "THROWN",
    note: "RangeError: Maximum loop iterations exceeded (catchable)",
  },
  {
    id: "oom-array",
    title: "Sorted-array materialization above its limit",
    code: "Array.prototype.toSorted.call({ length: 262145 });",
    opts: { memLimitKb: 2_097_152, timeoutMs: 30_000 },
    expected: "THROWN",
    note: "toSorted rejects materialization above 262,144 slots before reading entries",
  },
  {
    id: "oom-string",
    title: "Unbounded string doubling",
    code: "let s = 'x'; while (true) { s = s + s; }",
    opts: { memLimitKb: 2_097_152, timeoutMs: 30_000 },
    expected: "THROWN",
    note: "RangeError when doubling exceeds 64 Mi UTF-16 code units",
  },
  {
    id: "json-array-element-budget",
    title: "JSON array-element budget across duplicate object keys",
    code: `
      let part = '"a":[' + '0,'.repeat(32767) + '0],';
      JSON.parse('{' + part.repeat(128) + '"a":[0]}');
    `,
    opts: { memLimitKb: 2_097_152, timeoutMs: 30_000 },
    expected: "THROWN",
    note: "4 Mi aggregate parsed-array elements; duplicate keys keep live storage small",
  },
  {
    id: "shared-result-formatting",
    title: "Shared graph result-formatting amplification",
    code: `
      let child = ['x'.repeat(1024 * 1024)];
      let outer = [];
      for (let i = 0; i < 65; i++) outer.push(child);
      outer;
    `,
    opts: { memLimitKb: 2_097_152, timeoutMs: 30_000 },
    expected: "THROWN",
    note: "bounded incremental rendering raises RangeError before output amplification can OOM",
  },
  {
    id: "large-string-early-break",
    title: "Large string, first iterator item only",
    code: `
      let s = 'x'.repeat(16 * 1024 * 1024);
      let first = '';
      for (const c of s) { first = c; break; }
      first;
    `,
    opts: { memLimitKb: 2_097_152, timeoutMs: 30_000 },
    expected: "SURVIVED",
    note: "lazy Unicode-scalar iterator does not build a character array",
  },
  {
    id: "global-binding-exhaustion",
    title: "Dynamic global binding exhaustion",
    code: `
      try {
        for (let i = 0; i < 300000; i++) globalThis['guest_' + i] = i;
      } catch (e) { e.message; }
    `,
    opts: { memLimitKb: 2_097_152, timeoutMs: 30_000 },
    expected: "SURVIVED",
    note: "persistent global scope raises a catchable binding-count RangeError",
  },
  {
    id: "indexed-array-cap",
    title: "Indexed assignment remains sparse beyond the dense growth threshold",
    code: `
      let a = []; a[1000000000] = 1;
      if (a.length !== 1000000001 || a[1000000000] !== 1 || Object.keys(a).length !== 1)
        throw new Error('Sparse array invariant failed');
      'sparse';
    `,
    expected: "SURVIVED",
    note: "large logical length stores one indexed slot without allocating intervening holes",
  },
  {
    id: "iterative-flat",
    title: "100k-deep Array.prototype.flat",
    code: "let a = [0]; for (let i = 0; i < 100000; i++) { a = [a]; } a.flat(100000);",
    opts: { timeoutMs: 30_000 },
    expected: "SURVIVED",
    note: "flat uses an explicit work stack instead of native recursion",
  },
  {
    id: "sort-reentry",
    title: "Array.sort comparator mutates its array",
    code: "let a = [3, 2, 1]; a.sort((x, y) => { a.push(4); return x - y; }); a.length;",
    expected: "SURVIVED",
    note: "sort releases the RefCell borrow while calling guest comparators",
  },

  // ── Cases the VM already survives ──────────────────────────────────
  // These document containment that works today, so a regression shows
  // up as a SURVIVED → CRASHED flip.
  {
    id: "generator-deep-yieldstar",
    title: "Recursive yield* inside a generator",
    code: "function* g() { yield* g(); } let it = g(); it.next(); 'ok';",
    expected: "THROWN",
    note: "dedicated generator-nesting cap raises a catchable RangeError (each yield* level runs on its own coroutine stack, so MAX_CALL_DEPTH never sees it)",
  },
  {
    id: "generator-abandon",
    title: "Abandon a running generator",
    code: "function* g() { while (true) { yield 1; } } let it = g(); it.next(); it = null; 'ok';",
    expected: "SURVIVED",
    note: "dropping a live generator thread is clean",
  },
  {
    id: "runtime-error",
    title: "Plain runtime error (baseline)",
    code: "try { undefinedVar.foo; } catch (e) { 'caught: ' + e.message; }",
    expected: "SURVIVED",
    note: "runtime errors surface as catchable VM exceptions — as they should",
  },
  {
    id: "host-function-throw",
    title: "Exposed host function throws",
    code: "__HOST_CASE__", // special-cased in the child: needs exposeFunction
    expected: "SURVIVED",
    note: "Node-side throws cross back as catchable VM exceptions",
  },
];

// ── Child mode: run one case and report ──────────────────────────────

async function runChild(caseId: string): Promise<void> {
  const c = CASES.find((x) => x.id === caseId);
  if (!c) {
    console.error(`unknown case: ${caseId}`);
    process.exit(2);
  }
  // Lazy-require so `--list` never touches the native binding.
  const { Vm } = require("../index.js");
  const vm = new Vm();
  try {
    if (c.id === "host-function-throw") {
      vm.exposeFunction("boom", () => {
        throw new Error("from node");
      });
      const r = vm.run("try { boom(); } catch (e) { 'caught: ' + e.message; }");
      console.log(`RESULT: SURVIVED (${r})`);
    } else {
      const r = c.asyncRun ? await vm.runAsync(c.code) : vm.run(c.code);
      console.log(`RESULT: SURVIVED (${String(r).slice(0, 60)})`);
    }
  } catch (e) {
    const msg = String((e as Error)?.message ?? e).slice(0, 80);
    console.log(`RESULT: THROWN (${msg})`);
  }
  // If we get here the case was catchable. Deliberately no process.exit(): the
  // process tears down naturally so Rust destructors run — if a future
  // regression makes teardown crash (as deep-nesting-drop once did), the
  // parent still sees it through the exit status and scores it CRASHED.
}

// ── Parent mode: spawn each case, classify, tabulate ─────────────────

type Verdict = "SURVIVED" | "THROWN" | "CRASHED" | "HANG";

interface Row {
  c: CrashCase;
  verdict: Verdict;
  detail: string;
  ms: number;
}

const SIGNALS: Record<number, string> = {
  133: "SIGTRAP (OOM trap)",
  134: "SIGABRT (alloc failure)",
  139: "SIGSEGV (stack overflow)",
};

/**
 * The child runner: plain CJS piped into `node -e`. It reads one JSON line
 * from stdin ({ id, code, hostThrow, asyncRun }), executes it in a fresh Vm, and prints
 * `RESULT: SURVIVED|THROWN (…)`. If the process dies before printing, the
 * parent knows the guest code killed it.
 */
const RUNNER_JS = `
const { Vm } = require(process.env.VM_INDEX);
let data = "";
process.stdin.on("data", (d) => (data += d));
process.stdin.on("end", async () => {
  const { code, hostThrow, asyncRun } = JSON.parse(data);
  const vm = new Vm();
  try {
    if (hostThrow) {
      vm.exposeFunction("boom", () => { throw new Error("from node"); });
      const r = vm.run("try { boom(); } catch (e) { 'caught: ' + e.message; }");
      console.log("RESULT: SURVIVED (" + r + ")");
    } else {
      const r = asyncRun ? await vm.runAsync(code) : vm.run(code);
      console.log("RESULT: SURVIVED (" + String(r).slice(0, 60) + ")");
    }
  } catch (e) {
    console.log("RESULT: THROWN (" + String((e && e.message) || e).slice(0, 80) + ")");
  }
  // Deliberately NO process.exit(): we let the process tear down naturally so
  // Rust destructors run. If teardown ever crashes again (as deep-nesting-drop
  // once did, posthumously), the parent scores it CRASHED via the exit status.
});
`;

function runCase(c: CrashCase): Promise<Row> {
  return new Promise((resolve) => {
    const timeoutMs = c.opts?.timeoutMs ?? 10_000;
    const indexJs = require("node:path").resolve(import.meta.dir, "..", "index.js");
    const nodeArgs = ["-e", RUNNER_JS];
    let child;
    if (c.opts?.memLimitKb && process.platform === "linux") {
      // `ulimit` is a shell builtin, so the cap needs a shell wrapper:
      // `exec "$0" "$@"` makes node replace the shell, keeping signals clean.
      child = spawn(
        "sh",
        ["-c", `ulimit -v ${c.opts.memLimitKb} && exec "$0" "$@"`, "node", ...nodeArgs],
        { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, VM_INDEX: indexJs } }
      );
    } else {
      child = spawn("node", nodeArgs, {
        stdio: ["pipe", "pipe", "pipe"],
        env: { ...process.env, VM_INDEX: indexJs },
      });
    }

    // The guest code is passed via stdin (env vars have a ~128KB cap and the
    // deep-parse case is 200KB of parentheses).
    child.stdin.on("error", () => {}); // EPIPE if the child dies before reading
    child.stdin.end(
      JSON.stringify({
        id: c.id,
        code: c.code,
        hostThrow: c.code === "__HOST_CASE__",
        asyncRun: c.asyncRun === true,
      }),
    );

    let out = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (out += d));
    const t0 = Date.now();
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);

    child.on("close", (code, signal) => {
      clearTimeout(timer);
      const ms = Date.now() - t0;
      let verdict: Verdict;
      let detail: string;
      if (timedOut) {
        verdict = "HANG";
        detail = `killed after ${timeoutMs}ms`;
      } else if (code === 0) {
        const m = /RESULT: (SURVIVED|THROWN)(.*)/.exec(out);
        verdict = m?.[1] === "THROWN" ? "THROWN" : "SURVIVED";
        detail = (m?.[2] ?? "").replace(/^\s*\(|\)\s*$/g, "").slice(0, 50);
      } else if (code !== null && code > 128) {
        verdict = "CRASHED";
        detail = SIGNALS[code] ?? `signal ${code - 128}`;
      } else if (signal) {
        verdict = "CRASHED";
        detail = `signal ${signal}`;
      } else {
        verdict = "CRASHED";
        detail = `exit ${code}`;
      }
      resolve({ c, verdict, detail, ms });
    });
  });
}

const ICON: Record<Verdict, string> = {
  SURVIVED: "✅",
  THROWN: "✅", // a catchable throw IS the correct containment
  CRASHED: "💥",
  HANG: "🕐",
};

async function main(cases: CrashCase[] = CASES): Promise<void> {
  console.log("=== napi-vm Crash-Safety Matrix ===\n");
  console.log("Each case runs in a disposable subprocess; the verdict says how it died.\n");

  const rows: Row[] = [];
  for (const c of cases) {
    process.stdout.write(`  running ${c.id} …`);
    const row = await runCase(c);
    rows.push(row);
    // Erase the progress line, print the result line.
    process.stdout.write("\r\x1b[K");
    console.log(
      `  ${ICON[row.verdict]} ${row.verdict.padEnd(8)} ${row.c.id.padEnd(26)} ${row.detail} (${row.ms}ms)`
    );
  }

  // ── Summary ────────────────────────────────────────────────────────
  const crashed = rows.filter((r) => r.verdict === "CRASHED");
  const hung = rows.filter((r) => r.verdict === "HANG");
  const ok = rows.filter((r) => r.verdict === "SURVIVED" || r.verdict === "THROWN");

  console.log("\n--- Summary ---\n");
  console.log(`  contained (survived / catchable throw): ${ok.length}`);
  console.log(`  process killers (crash):                ${crashed.length}`);
  console.log(`  event-loop blockers (hang):             ${hung.length}`);

  const mismatches = rows.filter((r) => r.verdict !== r.c.expected);
  if (mismatches.length) {
    console.log("\n  verdicts that disagree with the case's `expected` field:");
    for (const r of mismatches) {
      console.log(`    ${r.c.id}: expected ${r.c.expected}, got ${r.verdict}`);
    }
  }

  if (crashed.length || hung.length || mismatches.length) {
    console.log("\n  Guest code CAN still take down the host (or a guard regressed).");
    console.log("  See the README under 'Sandbox limits & crash safety'.");
    process.exitCode = 1;
  } else {
    console.log("\n  All cases contained — every verdict matches `expected`.");
  }
  console.log("");
}

// ── Entry point ──────────────────────────────────────────────────────

const args = process.argv.slice(2);
if (args[0] === "--case" && args[1]) {
  // Single case, in-process under bun — for quick debugging. WARNING: a
  // CRASHED case will kill this process (that is the point), and bun's
  // SIGSEGV handler may turn crashes into hangs; use `node` + gdb for
  // serious triage, e.g.:  gdb --args node -e "$RUNNER" < case.json
  void runChild(args[1]);
} else if (args[0] === "--list") {
  for (const c of CASES) console.log(`${c.id.padEnd(28)} ${c.title}`);
} else if (args[0] === "--only") {
  if (!args[1]) throw new Error("--only requires comma-separated case IDs");
  const requested = new Set(args[1].split(","));
  const selected = CASES.filter((c) => requested.delete(c.id));
  if (requested.size) throw new Error(`Unknown crash case(s): ${[...requested].join(", ")}`);
  void main(selected);
} else {
  main();
}
