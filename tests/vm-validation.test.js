import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";

test("optional chaining accepts keyword property names", () => {
  expect(runCode("({ get: 42 })?.get;")).toBe("42");
});

test("object literals accept reserved words as property names", () => {
  expect(runCode("({ default: 42, class: 3 }).default + ({ class: 3 }).class;")).toBe("45");
});

test("contextual words can be function and variable names", () => {
  expect(runCode("function set() { return 42; } const async = set(); async;")).toBe("42");
});

test("Object.prototype.hasOwnProperty.call is available", () => {
  expect(runCode("Object.prototype.hasOwnProperty.call({ value: 1 }, 'value');")).toBe("true");
});

test("validateModule accepts module syntax without resolving imports", () => {
  const vm = new Vm();

  expect(
    vm.validateModule('import { value } from "not-registered"; export const result = value;'),
  ).toEqual({ valid: true, diagnostics: [] });
  expect(vm.hasModule("not-registered")).toBe(false);
});

test("validateModule parses source without executing it", () => {
  const vm = new Vm();

  expect(vm.validateModule('globalThis.touched = true; throw new Error("no");').valid).toBe(true);
  expect(vm.hasGlobal("touched")).toBe(false);
});

test("validateModule returns a positioned syntax diagnostic", () => {
  const result = new Vm().validateModule("\nconst value = ;");

  expect(result.valid).toBe(false);
  expect(result.diagnostics).toHaveLength(1);
  expect(result.diagnostics[0]).toMatchObject({
    line: 2,
    column: 15,
    kind: "syntax",
  });
  expect(result.diagnostics[0].message).toContain("unexpected token");
});

test("validateModule classifies excessive nesting as a parse limit", () => {
  const source = "(".repeat(300) + "1" + ")".repeat(300);
  const result = new Vm().validateModule(source);

  expect(result.valid).toBe(false);
  expect(result.diagnostics[0]).toMatchObject({
    kind: "parse-limit",
    message: "Maximum parse depth exceeded",
  });
});
