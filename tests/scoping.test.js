import { test, expect } from "bun:test";
import { Vm } from "../index.js";

/**
 * Lexical scoping: `let`/`const`/`var`, the temporal dead zone, hoisting, and
 * per-iteration loop bindings.
 *
 * These were previously all one thing — the interpreter matched `VarKind` as
 * `kind: _` and ran every block in its enclosing environment, so `let`, `const`
 * and `var` behaved identically and blocks introduced no scope.
 */

function run(source) {
  return new Vm().run(source);
}

function runFails(source) {
  return () => new Vm().run(source);
}

// ── block scope ──────────────────────────────────────────────────────

test("let is scoped to its block", () => {
  expect(run(`{ let x = 1; } typeof x;`)).toBe("undefined");
});

test("const is scoped to its block", () => {
  expect(run(`{ const k = 1; } typeof k;`)).toBe("undefined");
});

test("var ignores blocks and reaches the function scope", () => {
  expect(run(`{ var y = 1; } y;`)).toBe("1");
  expect(run(`function f(){ { var m = 1; } return m; } f();`)).toBe("1");
});

test("let inside a function block does not escape it", () => {
  expect(run(`function f(){ { let m = 1; } return typeof m; } f();`)).toBe("undefined");
});

test("block function declarations remain hoisted and block-scoped", () => {
  expect(run(`let result; { result = local(); function local(){ return 4; } } result;`)).toBe("4");
  expect(run(`function outer(){ { function local(){ return 4; } } return typeof local; } outer();`)).toBe(
    "undefined",
  );
});

test("an inner block shadows rather than overwrites", () => {
  expect(run(`let s = 1; { let s = 2; } s;`)).toBe("1");
  expect(run(`let d = 1; { let d = 2; { let d = 3; } } d;`)).toBe("1");
});

test("assignment without a declaration reaches the outer binding", () => {
  expect(run(`let o = 1; { o = 2; } o;`)).toBe("2");
});

test("if, loop and for-of bodies are blocks", () => {
  expect(run(`if (true) { let b = 1; } typeof b;`)).toBe("undefined");
  expect(run(`let n = 0; while (n < 1) { let z = 1; n++; } typeof z;`)).toBe("undefined");
  expect(run(`for (const v of [1]) { let t = 1; } typeof t;`)).toBe("undefined");
});

test("a catch parameter is scoped to its clause", () => {
  expect(run(`try { throw 1; } catch (e) {} typeof e;`)).toBe("undefined");
});

test("switch cases share one block scope", () => {
  // Fall-through means a `let` from one case is visible in the next.
  expect(run(`let out = 0; switch (1) { case 1: let sv = 5; case 2: out = sv; } out;`)).toBe("5");
});

test("a closure captures its block's binding", () => {
  expect(run(`let fn; { let c = 9; fn = () => c; } fn();`)).toBe("9");
});

// ── const ────────────────────────────────────────────────────────────

test("const rejects reassignment", () => {
  expect(runFails(`const c = 1; c = 2;`)).toThrow(
    /TypeError: Assignment to constant variable 'c'/,
  );
});

test("const rejects compound assignment and increment", () => {
  expect(runFails(`const c = 1; c += 1;`)).toThrow(/Assignment to constant variable/);
  expect(runFails(`const c = 1; c++;`)).toThrow(/Assignment to constant variable/);
  expect(runFails(`const c = 1; --c;`)).toThrow(/Assignment to constant variable/);
});

test("const binds, it does not freeze", () => {
  expect(run(`const obj = { a: 1 }; obj.a = 2; obj.a;`)).toBe("2");
});

test("a destructured const is still const", () => {
  expect(runFails(`const { p } = { p: 1 }; p = 2;`)).toThrow(
    /Assignment to constant variable/,
  );
  expect(run(`let [q] = [1]; q = 2; q;`)).toBe("2");
});

test("let remains reassignable", () => {
  expect(run(`let l = 1; l = 2; l;`)).toBe("2");
});

// ── temporal dead zone ───────────────────────────────────────────────

test("reading a let before its declaration is a TDZ error", () => {
  expect(runFails(`{ x; let x = 1; }`)).toThrow(
    /ReferenceError: Cannot access 'x' before initialization/,
  );
});

test("writing to a let before its declaration is a TDZ error", () => {
  expect(runFails(`{ x = 5; let x = 1; }`)).toThrow(
    /Cannot access 'x' before initialization/,
  );
});

test("const has a dead zone too", () => {
  expect(runFails(`{ y; const y = 1; }`)).toThrow(
    /Cannot access 'y' before initialization/,
  );
});

test("the dead zone is distinct from an undeclared name", () => {
  expect(runFails(`nope;`)).toThrow(/ReferenceError: nope is not defined/);
});

// ── hoisting ─────────────────────────────────────────────────────────

test("function declarations are hoisted and callable above themselves", () => {
  expect(run(`let r = f(); function f(){ return 7; } r;`)).toBe("7");
});

test("var is hoisted as undefined, not left undeclared", () => {
  expect(run(`let r = typeof v; var v = 1; r;`)).toBe("undefined");
});

test("a bare var redeclaration does not erase the value", () => {
  expect(run(`var a = 1; var a; a;`)).toBe("1");
});

test("var declared inside a nested block hoists to the function", () => {
  expect(run(`function f(){ let seen = typeof inner; { var inner = 1; } return seen; } f();`)).toBe(
    "undefined",
  );
});

// ── per-iteration bindings ───────────────────────────────────────────

test("for(let) gives each iteration its own binding", () => {
  expect(
    run(`const fs = []; for (let i = 0; i < 3; i++) fs.push(() => i); fs.map(f => f()).join(",");`),
  ).toBe("0,1,2");
});

test("for(let) captures from the initializer and update keep their iteration bindings", () => {
  expect(
    run(
      `let first; for (let i = 0, capture = () => i; i < 2; i++) { if (i === 0) first = capture; } first();`,
    ),
  ).toBe("0");
  expect(
    run(
      `const captures = []; for (let i = 0; i < 2; captures.push(() => i), i++) {} captures.map(f => f()).join(",");`,
    ),
  ).toBe("1,2");
});

test("for(var) keeps one shared binding", () => {
  expect(
    run(`const fs = []; for (var i = 0; i < 3; i++) fs.push(() => i); fs.map(f => f()).join(",");`),
  ).toBe("3,3,3");
});

test("a for(let) binding does not leak, a for(var) one does", () => {
  expect(run(`for (let q = 0; q < 1; q++) {} typeof q;`)).toBe("undefined");
  expect(run(`for (var w = 0; w < 1; w++) {} w;`)).toBe("1");
});

// ── multiple declarators ─────────────────────────────────────────────

test("multiple declarators bind in the enclosing scope", () => {
  // These are parsed as one grouping node, which must not introduce a scope.
  expect(run(`let a = 1, b = 2; a + b;`)).toBe("3");
  expect(run(`const c = 1, d = 2; c + d;`)).toBe("3");
  expect(run(`var e = 1, f = 2; e + f;`)).toBe("3");
});

// ── VM session semantics ─────────────────────────────────────────────

test("top-level let and const persist across run() calls", () => {
  const vm = new Vm();
  vm.run(`let counter = 1; const label = "x";`);
  expect(vm.run(`counter;`)).toBe("1");
  expect(vm.run(`label;`)).toBe("x");
  vm.run(`counter = 2;`);
  expect(vm.run(`counter;`)).toBe("2");
});

test("a top-level const stays const across run() calls", () => {
  const vm = new Vm();
  vm.run(`const frozen = 1;`);
  expect(() => vm.run(`frozen = 2;`)).toThrow(/Assignment to constant variable/);
});
