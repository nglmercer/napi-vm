import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";
import { runtimeFixture } from "./runtime-fixture.mjs";

// The core exposes ECMAScript intrinsics, never inert placeholders for host APIs.
for (const name of ["require", "process", "window", "self", "fetch", "setTimeout", "setInterval", "queueMicrotask", "console", "Worker", "SharedWorker", "__dirname", "__filename", "GPU", "GPUDevice", "GPUBuffer", "ReadableStream", "WritableStream", "TransformStream", "ServiceWorker", "ServiceWorkerContainer", "ServiceWorkerRegistration", "Event", "EventTarget", "CustomEvent", "AbortController", "TextEncoder", "TextDecoder", "Blob", "File", "FormData", "structuredClone", "indexedDB", "caches", "sessionStorage"]) {
  test(`${name} is absent before runtime or host installation`, () => {
    expect(runCode(`typeof ${name};`)).toBe("undefined");
  });
}

test("globalThis is the guest global object", () => {
  expect(runCode("typeof globalThis;")).toBe("object");
});

test("eval executes inside the guest realm", () => {
  expect(runCode("typeof eval;")).toBe("function");
  expect(runCode("eval('typeof process');")).toBe("undefined");
});

test("vm instances are isolated", () => {
  const vm1 = new Vm();
  const vm2 = new Vm();
  vm1.run("const secret = 'vm1';");
  expect(() => vm2.run("secret;")).toThrow();
});

test("vm state does not leak between instances", () => {
  const vm1 = new Vm();
  const vm2 = new Vm();
  vm1.run("let counter = 100;");
  vm2.run("let counter = 0;");
  expect(vm1.run("counter;")).toBe("100");
  expect(vm2.run("counter;")).toBe("0");
});

test("runCode is stateless between calls", () => {
  runCode("const x = 42;");
  expect(() => runCode("x;")).toThrow();
});

test("cannot escape sandbox via the Function constructor", () => {
  const vm = new Vm();
  // `Function` compiles source, but into *this* interpreter: the classic
  // escape works by reaching a host realm, and there is none to reach.
  expect(vm.run("typeof Function;")).toBe("function");
  expect(vm.run("typeof new Function('return this')();")).toBe("object");
  expect(vm.run("new Function('return typeof Deno')();")).toBe("undefined");
  expect(vm.run("new Function('return typeof globalThis.process')();")).toBe(
    "undefined",
  );
  // Constructor chains resolve to guest intrinsics. Dynamic source still
  // runs in this interpreter and cannot reach a host realm.
  expect(vm.run("(function () {}).constructor === Function;")).toBe("true");
  expect(vm.run("(function () {}).constructor.constructor === Function;")).toBe("true");
  expect(
    vm.run("(function () {}).constructor.constructor('return typeof Deno')();"),
  ).toBe("undefined");
  expect(vm.run("[].constructor === Array;")).toBe("true");
  expect(vm.run("[].constructor.constructor === Function;")).toBe("true");
});

test("Proxy intercepts guest objects only", () => {
  const vm = new Vm();
  expect(vm.run("typeof Proxy;")).toBe("function");
  // A proxy can only wrap a value the guest already holds, so it grants no
  // reach the guest did not have.
  expect(() => vm.run("new Proxy(1, {});")).toThrow();
  expect(vm.run("new Proxy({ a: 1 }, {}).a;")).toBe("1");
});

const webRuntime = runtimeFixture({ webApis: true });
webRuntime.test("explicit structuredClone copies guest values without host reach", () => {
  expect(webRuntime.run("structuredClone({ a: 1 }).a;")).toBe("1");
  expect(webRuntime.run("typeof process;")).toBe("undefined");
});

const timersRuntime = runtimeFixture({ timers: true, clock: "virtual" });
timersRuntime.test("explicit queueMicrotask runs on the owner's queue", () => {
  expect(timersRuntime.run("var out=[]; queueMicrotask(() => out.push('later')); out.push('now'); await 0; out.join();")).toBe("now,later");
});
