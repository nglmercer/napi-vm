#!/usr/bin/env node
// Dependency-free end-to-end benchmark for the VM through its NAPI binding.
//
// Each workload is run through `runCode` (full lex → parse → eval, crossing the
// NAPI boundary on every call) and, where a clean analog exists, as native JS.
// The ratio gives a rough sense of the interpreter's overhead versus the host
// engine. Run with `npm run bench` (or `bun bench/bench.js`).

"use strict";

const { runCode } = process.env.NAPI_BENCH_BINDING
  ? require(require("node:path").resolve(process.env.NAPI_BENCH_BINDING))
  : require("../index.js");

// How long to spend measuring each workload, in milliseconds.
const MIN_MS = 250;
const WARMUP_ITERS = 20;

// Workloads mirror benches/vm.rs so the two layers stay comparable. `native`
// is an optional function returning a comparable result, used as a baseline.
const WORKLOADS = [
  {
    name: "arithmetic_loop",
    src: "let s = 0; for (let i = 0; i < 10000; i++) { s += i * 2 - 1; } s;",
    native: () => {
      let s = 0;
      for (let i = 0; i < 10000; i++) s += i * 2 - 1;
      return s;
    },
  },
  {
    name: "recursion_fib",
    src: "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(10);",
    native: () => {
      const fib = (n) => (n < 2 ? n : fib(n - 1) + fib(n - 2));
      return fib(10);
    },
  },
  {
    name: "array_chain",
    src:
      "let a = []; for (let i = 0; i < 1000; i++) { a.push(i); } " +
      "a.map(x => x * 2).filter(x => x % 3 === 0).reduce((s, x) => s + x, 0);",
    native: () => {
      const a = [];
      for (let i = 0; i < 1000; i++) a.push(i);
      return a.map((x) => x * 2).filter((x) => x % 3 === 0).reduce((s, x) => s + x, 0);
    },
  },
  {
    name: "string_ops",
    src:
      "let parts = []; for (let i = 0; i < 1000; i++) { parts.push('item' + i); } " +
      "parts.join(',').split(',').length;",
    native: () => {
      const parts = [];
      for (let i = 0; i < 1000; i++) parts.push("item" + i);
      return parts.join(",").split(",").length;
    },
  },
  {
    name: "class_methods",
    src:
      "class P { constructor(x, y) { this.x = x; this.y = y; } sum() { return this.x + this.y; } } " +
      "let t = 0; for (let i = 0; i < 1000; i++) { t += new P(i, i + 1).sum(); } t;",
    native: () => {
      class P {
        constructor(x, y) {
          this.x = x;
          this.y = y;
        }
        sum() {
          return this.x + this.y;
        }
      }
      let t = 0;
      for (let i = 0; i < 1000; i++) t += new P(i, i + 1).sum();
      return t;
    },
  },
  {
    name: "closures",
    src:
      "function counter() { let n = 0; return () => ++n; } " +
      "const c = counter(); for (let i = 0; i < 10000; i++) { c(); } c();",
    native: () => {
      const counter = () => {
        let n = 0;
        return () => ++n;
      };
      const c = counter();
      for (let i = 0; i < 10000; i++) c();
      return c();
    },
  },
  {
    name: "json_roundtrip",
    src:
      "const o = { a: 1, b: [1, 2, 3], c: { d: 'x', e: [true, null] } }; " +
      "let r; for (let i = 0; i < 200; i++) { r = JSON.parse(JSON.stringify(o)); } " +
      "r.c.e.length + r.b.length;",
    native: () => {
      const o = { a: 1, b: [1, 2, 3], c: { d: "x", e: [true, null] } };
      let r;
      for (let i = 0; i < 200; i++) r = JSON.parse(JSON.stringify(o));
      return r.c.e.length + r.b.length;
    },
  },
];

// Time `fn` until at least `minMs` has elapsed; returns per-op stats.
function measure(fn, minMs) {
  for (let i = 0; i < WARMUP_ITERS; i++) fn();
  let iters = 0;
  const start = performance.now();
  do {
    fn();
    iters++;
  } while (performance.now() - start < minMs);
  const ms = performance.now() - start;
  return {
    iters,
    nsPerOp: (ms * 1e6) / iters,
    opsPerSec: iters / (ms / 1000),
  };
}

function fmtNum(n) {
  if (n >= 1e6) return (n / 1e6).toFixed(2) + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(2) + "k";
  return n.toFixed(0);
}

function fmtNs(ns) {
  if (ns >= 1e6) return (ns / 1e6).toFixed(2) + " ms";
  if (ns >= 1e3) return (ns / 1e3).toFixed(2) + " µs";
  return ns.toFixed(1) + " ns";
}

function main() {
  const engine = typeof Bun !== "undefined" ? "Bun" : "Node " + process.version;
  console.log(`napi-vm end-to-end benchmark (${engine}, ${process.platform}/${process.arch})`);
  console.log(`Measuring each workload for >= ${MIN_MS} ms after ${WARMUP_ITERS} warmup iters.\n`);

  const header =
    "workload".padEnd(18) +
    "vm/op".padStart(12) +
    "vm ops/s".padStart(12) +
    "native/op".padStart(12) +
    "ratio".padStart(10);
  console.log(header);
  console.log("-".repeat(header.length));

  for (const w of WORKLOADS) {
    // Correctness guard: the VM result must match the native baseline.
    if (w.native) {
      const vmOut = runCode(w.src);
      const nativeOut = String(w.native());
      if (vmOut !== nativeOut) {
        throw new Error(`${w.name}: result mismatch vm=${vmOut} native=${nativeOut}`);
      }
    }

    const vm = measure(() => runCode(w.src), MIN_MS);
    let native = null;
    if (w.native) native = measure(w.native, MIN_MS);

    const ratio = native ? (vm.nsPerOp / native.nsPerOp).toFixed(0) + "x" : "-";
    console.log(
      w.name.padEnd(18) +
        fmtNs(vm.nsPerOp).padStart(12) +
        fmtNum(vm.opsPerSec).padStart(12) +
        (native ? fmtNs(native.nsPerOp).padStart(12) : "-".padStart(12)) +
        ratio.padStart(10),
    );
  }

  console.log("\nratio = vm time / native time (higher = slower than the host engine)");
}

main();
