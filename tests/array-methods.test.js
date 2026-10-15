import { test, expect } from "bun:test";
import { runCode } from "../index.js";
import { hasTrueSuspension } from "./suspension.mjs";

// ---------------------------------------------------------------------------
// `Array.prototype` methods added alongside the rest, and the generator
// methods (`throw`, `return`) that complete the iterator protocol.
// ---------------------------------------------------------------------------

test("slice rejects result lengths above 2^32-1", () => {
  expect(
    runCode(
      "try { Array.prototype.slice.call({ length: 4294967296 }); 'no throw'; } catch (e) { e.name; }"
    )
  ).toBe("RangeError");
  expect(runCode("[1, 2, 3].slice(1).join(',');")).toBe("2,3");
});

test("splice rejects result lengths above 2^32-1", () => {
  expect(
    runCode(
      "try { Array.prototype.splice.call({ length: 4294967296 }, 0, 4294967296); 'no throw'; } catch (e) { e.name; }"
    )
  ).toBe("RangeError");
  expect(runCode("let a = [1, 2, 3]; a.splice(1, 1); a.join(',');")).toBe("1,3");
});

test("at supports negative indices", () => {
  expect(runCode("[1, 2, 3].at(-1);")).toBe("3");
  expect(runCode("[1, 2, 3].at(0);")).toBe("1");
  expect(runCode("String([1].at(5));")).toBe("undefined");
});

test("findIndex reports the position", () => {
  expect(runCode("[1, 2, 3].findIndex((x) => x > 1);")).toBe("1");
  expect(runCode("[1].findIndex((x) => x > 9);")).toBe("-1");
});

test("findLast and findLastIndex search from the end", () => {
  expect(runCode("[1, 2, 3].findLast((x) => x < 3);")).toBe("2");
  expect(runCode("[1, 2, 3].findLastIndex((x) => x < 3);")).toBe("1");
});

test("lastIndexOf searches from the end", () => {
  expect(runCode("[1, 2, 1].lastIndexOf(1);")).toBe("2");
  expect(runCode("[1].lastIndexOf(9);")).toBe("-1");
});

test("shift and unshift work at the front", () => {
  expect(runCode("const a = [1, 2]; a.shift(); a.join();")).toBe("2");
  expect(runCode("const a = [1, 2]; a.shift();")).toBe("1");
  expect(runCode("const a = [2]; a.unshift(1); a.join();")).toBe("1,2");
  expect(runCode("const a = [2]; a.unshift(1);")).toBe("2");
  expect(runCode("String([].shift());")).toBe("undefined");
});

test("fill writes a range", () => {
  expect(runCode("[1, 2, 3].fill(0).join();")).toBe("0,0,0");
  expect(runCode("[1, 2, 3].fill(0, 1).join();")).toBe("1,0,0");
  expect(runCode("[1, 2, 3].fill(0, 1, 2).join();")).toBe("1,0,3");
  expect(runCode("[1, 2, 3].fill(0, -1).join();")).toBe("1,2,0");
});

test("keys, values and entries iterate", () => {
  expect(runCode("[...[1, 2].keys()].join();")).toBe("0,1");
  expect(runCode("[...[1, 2].values()].join();")).toBe("1,2");
  expect(runCode("[...['a', 'b'].entries()].map((e) => e.join(':')).join();")).toBe("0:a,1:b");
});

// --- Generator throw / return -----------------------------------------------

test("gen.throw raises at the suspension point", () => {
  const source =
    "function* g() { try { yield 1; } catch (e) { yield 'caught ' + e; } } const it = g(); it.next(); it.throw('x').value;";
  if (!hasTrueSuspension()) {
    // Buffered target: nothing is suspended (the body already ran to
    // completion), so the value is re-thrown at the caller instead.
    expect(() => runCode(source)).toThrow("x");
    return;
  }
  expect(runCode(source)).toBe("caught x");
});

test("an uncaught gen.throw propagates to the caller", () => {
  expect(() =>
    runCode("function* g() { yield 1; } const it = g(); it.next(); it.throw(new Error('boom'));"),
  ).toThrow();
});

test("gen.return finishes the generator", () => {
  expect(
    runCode("function* g() { yield 1; yield 2; } const it = g(); it.next(); it.return(9).done;"),
  ).toBe("true");
  expect(
    runCode("function* g() { yield 1; yield 2; } const it = g(); it.next(); it.return(9).value;"),
  ).toBe("9");
});

test("gen.return runs a finally block", () => {
  expect(
    runCode(
      "let closed = false; function* g() { try { yield 1; } finally { closed = true; } } const it = g(); it.next(); it.return(); closed;",
    ),
  ).toBe("true");
});

test("a returned generator is exhausted", () => {
  expect(
    runCode(
      "function* g() { yield 1; yield 2; } const it = g(); it.next(); it.return(); String(it.next().done);",
    ),
  ).toBe("true");
});
