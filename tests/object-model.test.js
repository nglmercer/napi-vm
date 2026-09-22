import { test, expect } from "bun:test";
import { runCode } from "../index.js";

// ---------------------------------------------------------------------------
// Object model: property descriptors, `delete`, prototype semantics, and
// `Reflect`. Each expectation matches what the same source returns in a real
// JavaScript engine.
// ---------------------------------------------------------------------------

// --- delete -----------------------------------------------------------------

test("delete removes the property", () => {
  expect(runCode("const o = { a: 1, b: 2 }; delete o.a; JSON.stringify(o);")).toBe('{"b":2}');
});

test("object literal duplicate keys use the last value and keep insertion order", () => {
  expect(runCode("JSON.stringify({ a: 1, b: 2, a: 3 });")).toBe('{"a":3,"b":2}');
});

test("delete reports success", () => {
  expect(runCode("const o = { a: 1 }; delete o.a;")).toBe("true");
});

test("delete of a computed key", () => {
  expect(runCode("const o = { a: 1 }; const k = 'a'; delete o[k]; 'a' in o;")).toBe("false");
});

test("delete of a missing property still succeeds", () => {
  expect(runCode("const o = {}; delete o.nope;")).toBe("true");
});

test("delete of a non-reference is true", () => {
  expect(runCode("delete 42;")).toBe("true");
});

test("delete of a declared binding is false", () => {
  expect(runCode("let x = 1; delete x;")).toBe("false");
});

test("delete does not reach the prototype", () => {
  expect(
    runCode("const p = { a: 1 }; const c = Object.create(p); delete c.a; c.a;"),
  ).toBe("1");
});

test("delete of a non-configurable property fails", () => {
  expect(
    runCode("const o = {}; Object.defineProperty(o, 'a', { value: 1 }); delete o.a;"),
  ).toBe("false");
});

// --- `in` walks the prototype chain ----------------------------------------

test("in finds an own property", () => {
  expect(runCode("'a' in { a: 1 };")).toBe("true");
});

test("in finds an inherited property", () => {
  expect(runCode("const p = { a: 1 }; 'a' in Object.create(p);")).toBe("true");
});

test("in reports a missing property", () => {
  expect(runCode("'zz' in { a: 1 };")).toBe("false");
});

test("in works on array indices", () => {
  expect(runCode("0 in [7];")).toBe("true");
  expect(runCode("5 in [7];")).toBe("false");
});

// --- Reference identity -----------------------------------------------------

test("an object is strictly equal to itself", () => {
  expect(runCode("const o = {}; o === o;")).toBe("true");
});

test("distinct objects are not strictly equal", () => {
  expect(runCode("({}) === ({});")).toBe("false");
});

test("an array is strictly equal to itself", () => {
  expect(runCode("const a = [1]; a === a;")).toBe("true");
});

test("a function is strictly equal to itself", () => {
  expect(runCode("const f = () => 1; f === f;")).toBe("true");
});

test("aliases share identity", () => {
  expect(runCode("const a = { x: 1 }; const b = a; a === b;")).toBe("true");
});

// --- Object.create / prototypes --------------------------------------------

test("Object.create links the prototype", () => {
  expect(runCode("const p = { x: 1 }; Object.create(p).x;")).toBe("1");
});

test("Object.create leaves the prototype out of the own keys", () => {
  expect(runCode("const p = { x: 1 }; Object.keys(Object.create(p)).length;")).toBe("0");
});

test("Object.getPrototypeOf round-trips", () => {
  expect(runCode("const p = {}; Object.getPrototypeOf(Object.create(p)) === p;")).toBe("true");
});

test("Object.create(null) has a null prototype", () => {
  expect(runCode("Object.getPrototypeOf(Object.create(null)) === null;")).toBe("true");
});

test("Object.setPrototypeOf is observed through every reference", () => {
  expect(
    runCode("const o = {}; const alias = o; Object.setPrototypeOf(o, { y: 2 }); alias.y;"),
  ).toBe("2");
});

test("Object.create with a descriptor map", () => {
  expect(
    runCode("Object.create(null, { a: { value: 1, enumerable: true } }).a;"),
  ).toBe("1");
});

// --- Descriptors ------------------------------------------------------------

test("defineProperty defaults to non-enumerable", () => {
  expect(
    runCode("const o = {}; Object.defineProperty(o, 'a', { value: 1 }); JSON.stringify(o);"),
  ).toBe("{}");
});

test("a non-enumerable property is still readable", () => {
  expect(
    runCode("const o = {}; Object.defineProperty(o, 'a', { value: 1 }); o.a;"),
  ).toBe("1");
});

test("an enumerable defined property appears in Object.keys", () => {
  expect(
    runCode(
      "const o = {}; Object.defineProperty(o, 'a', { value: 1, enumerable: true }); Object.keys(o).join();",
    ),
  ).toBe("a");
});

test("defineProperty defaults to non-writable", () => {
  expect(
    runCode("const o = {}; Object.defineProperty(o, 'a', { value: 1 }); o.a = 5; o.a;"),
  ).toBe("1");
});

test("a writable defined property accepts assignment", () => {
  expect(
    runCode(
      "const o = {}; Object.defineProperty(o, 'a', { value: 1, writable: true }); o.a = 5; o.a;",
    ),
  ).toBe("5");
});

test("getOwnPropertyDescriptor of a plain property", () => {
  expect(
    runCode("JSON.stringify(Object.getOwnPropertyDescriptor({ a: 1 }, 'a'));"),
  ).toBe('{"value":1,"writable":true,"enumerable":true,"configurable":true}');
});

test("getOwnPropertyDescriptor of a missing property", () => {
  expect(runCode("Object.getOwnPropertyDescriptor({}, 'a');")).toBe("undefined");
});

test("getOwnPropertyDescriptors covers every own property", () => {
  expect(
    runCode("Object.keys(Object.getOwnPropertyDescriptors({ a: 1, b: 2 })).join();"),
  ).toBe("a,b");
});

test("defineProperty installs a getter", () => {
  expect(
    runCode("const o = {}; Object.defineProperty(o, 'a', { get() { return 7; } }); o.a;"),
  ).toBe("7");
});

test("defineProperty installs a setter", () => {
  expect(
    runCode(
      "let v = 0; const o = {}; Object.defineProperty(o, 'a', { set(x) { v = x; } }); o.a = 9; v;",
    ),
  ).toBe("9");
});

test("defineProperty installs a getter and a setter together", () => {
  expect(
    runCode(
      "let v = 0; const o = {}; Object.defineProperty(o, 'a', { get() { return v; }, set(x) { v = x * 2; } }); o.a = 5; o.a;",
    ),
  ).toBe("10");
});

test("object literal getter and setter declarations combine", () => {
  expect(
    runCode("let value = 0; const o = { get x() { return value; }, set x(n) { value = n; } }; o.x = 5; o.x;"),
  ).toBe("5");
});

test("object spread reads an accessor and copies its current value", () => {
  expect(
    runCode("let value = 1; const source = { get x() { return value; } }; const copy = { ...source }; value = 2; copy.x;"),
  ).toBe("1");
});

test("defineProperties installs several at once", () => {
  expect(
    runCode(
      "const o = Object.defineProperties({}, { a: { value: 1 }, b: { value: 2 } }); o.a + o.b;",
    ),
  ).toBe("3");
});

test("redefining a non-configurable property throws", () => {
  expect(() =>
    runCode(
      "const o = {}; Object.defineProperty(o, 'a', { value: 1 }); Object.defineProperty(o, 'a', { value: 2 });",
    ),
  ).toThrow();
});

test("getOwnPropertyNames includes non-enumerable properties", () => {
  expect(
    runCode(
      "const o = {}; Object.defineProperty(o, 'a', { value: 1 }); Object.getOwnPropertyNames(o).join();",
    ),
  ).toBe("a");
});

// --- Integrity levels -------------------------------------------------------

test("freeze rejects writes", () => {
  expect(runCode("const o = Object.freeze({ a: 1 }); o.a = 2; o.a;")).toBe("1");
});

test("freeze rejects additions", () => {
  expect(runCode("const o = Object.freeze({ a: 1 }); o.b = 2; JSON.stringify(o);")).toBe('{"a":1}');
});

test("isFrozen reports a frozen object", () => {
  expect(runCode("Object.isFrozen(Object.freeze({ a: 1 }));")).toBe("true");
});

test("isFrozen reports an ordinary object", () => {
  expect(runCode("Object.isFrozen({ a: 1 });")).toBe("false");
});

test("seal keeps writes but rejects additions", () => {
  expect(runCode("const o = Object.seal({ a: 1 }); o.a = 2; o.b = 3; JSON.stringify(o);")).toBe(
    '{"a":2}',
  );
});

test("isSealed reports a sealed object", () => {
  expect(runCode("Object.isSealed(Object.seal({ a: 1 }));")).toBe("true");
});

test("a sealed property cannot be deleted", () => {
  expect(runCode("const o = Object.seal({ a: 1 }); delete o.a;")).toBe("false");
});

test("preventExtensions blocks new properties", () => {
  expect(
    runCode("const o = {}; Object.preventExtensions(o); o.a = 1; Object.isExtensible(o);"),
  ).toBe("false");
});

// --- Object statics ---------------------------------------------------------

test("Object.hasOwn sees an own property", () => {
  expect(runCode("Object.hasOwn({ a: 1 }, 'a');")).toBe("true");
});

test("Object.hasOwn ignores the prototype", () => {
  expect(runCode("Object.hasOwn(Object.create({ a: 1 }), 'a');")).toBe("false");
});

test("Object.fromEntries builds an object", () => {
  expect(runCode("JSON.stringify(Object.fromEntries([['a', 1], ['b', 2]]));")).toBe(
    '{"a":1,"b":2}',
  );
});

test("Object.is distinguishes NaN", () => {
  expect(runCode("Object.is(NaN, NaN);")).toBe("true");
});

test("Object.is distinguishes signed zero", () => {
  expect(runCode("Object.is(0, -0);")).toBe("false");
});

test("Object.values runs getters", () => {
  expect(runCode("Object.values({ get a() { return 3; } }).join();")).toBe("3");
});

test("Object.entries pairs keys with values", () => {
  expect(runCode("JSON.stringify(Object.entries({ a: 1 }));")).toBe('[["a",1]]');
});

// --- Reflect ----------------------------------------------------------------

test("Reflect.get reads a property", () => {
  expect(runCode("Reflect.get({ a: 5 }, 'a');")).toBe("5");
});

test("Reflect.set writes a property", () => {
  expect(runCode("const o = {}; Reflect.set(o, 'k', 4); o.k;")).toBe("4");
});

test("Reflect.has follows the prototype chain", () => {
  expect(runCode("Reflect.has(Object.create({ a: 1 }), 'a');")).toBe("true");
});

test("Reflect.deleteProperty removes a property", () => {
  expect(runCode("const o = { a: 1 }; Reflect.deleteProperty(o, 'a'); JSON.stringify(o);")).toBe(
    "{}",
  );
});

test("Reflect.ownKeys lists own properties", () => {
  expect(runCode("Reflect.ownKeys({ a: 1, b: 2 }).join();")).toBe("a,b");
});

test("Reflect.apply calls with an argument array", () => {
  expect(runCode("Reflect.apply(function (a, b) { return a + b; }, null, [1, 2]);")).toBe("3");
});

test("Reflect.construct builds an instance", () => {
  expect(
    runCode("class A { constructor(x) { this.x = x; } } Reflect.construct(A, [3]).x;"),
  ).toBe("3");
});

test("Reflect.defineProperty reports failure instead of throwing", () => {
  expect(
    runCode(
      "const o = {}; Object.defineProperty(o, 'a', { value: 1 }); Reflect.defineProperty(o, 'a', { value: 2 });",
    ),
  ).toBe("false");
});

test("Reflect.getPrototypeOf matches Object.getPrototypeOf", () => {
  expect(runCode("Reflect.getPrototypeOf(Object.create(null)) === null;")).toBe("true");
});

test("Reflect.preventExtensions reports success", () => {
  expect(runCode("Reflect.preventExtensions({});")).toBe("true");
});

// --- `get` and `set` as ordinary property names -----------------------------

test("an object property may be named get", () => {
  expect(runCode("({ get: 1 }).get;")).toBe("1");
});

test("an object method may be named set", () => {
  expect(runCode("({ set(x) { return x + 1; } }).set(1);")).toBe("2");
});
