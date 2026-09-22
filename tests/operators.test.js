import { test, expect } from "bun:test";
import { runCode } from "../index.js";

test("strict equality ===", () => {
  expect(runCode("5 === 5;")).toBe("true");
  expect(runCode("5 === 6;")).toBe("false");
  expect(runCode("'a' === 'a';")).toBe("true");
  expect(runCode("'a' === 'b';")).toBe("false");
  expect(runCode("true === true;")).toBe("true");
  expect(runCode("true === false;")).toBe("false");
});

test("strict inequality !==", () => {
  expect(runCode("5 !== 3;")).toBe("true");
  expect(runCode("5 !== 5;")).toBe("false");
  expect(runCode("'a' !== 'b';")).toBe("true");
});

test("strict equality type mismatch", () => {
  expect(runCode("5 === '5';")).toBe("false");
  expect(runCode("0 === false;")).toBe("false");
  expect(runCode("'' === false;")).toBe("false");
  expect(runCode("null === undefined;")).toBe("false");
});

test("loose equality ==", () => {
  expect(runCode("5 == 5;")).toBe("true");
  expect(runCode("'a' == 'a';")).toBe("true");
  expect(runCode("true == true;")).toBe("true");
});

test("loose equality null/undefined", () => {
  expect(runCode("null == undefined;")).toBe("true");
  expect(runCode("undefined == null;")).toBe("true");
});

test("loose inequality !=", () => {
  expect(runCode("5 != 3;")).toBe("true");
  expect(runCode("5 != 5;")).toBe("false");
});

test("less than <", () => {
  expect(runCode("3 < 5;")).toBe("true");
  expect(runCode("5 < 3;")).toBe("false");
  expect(runCode("5 < 5;")).toBe("false");
});

test("greater than >", () => {
  expect(runCode("5 > 3;")).toBe("true");
  expect(runCode("3 > 5;")).toBe("false");
  expect(runCode("5 > 5;")).toBe("false");
});

test("less than or equal <=", () => {
  expect(runCode("3 <= 5;")).toBe("true");
  expect(runCode("5 <= 5;")).toBe("true");
  expect(runCode("6 <= 5;")).toBe("false");
});

test("greater than or equal >=", () => {
  expect(runCode("5 >= 3;")).toBe("true");
  expect(runCode("5 >= 5;")).toBe("true");
  expect(runCode("4 >= 5;")).toBe("false");
});

test("logical AND &&", () => {
  expect(runCode("true && true;")).toBe("true");
  expect(runCode("true && false;")).toBe("false");
  expect(runCode("false && true;")).toBe("false");
  expect(runCode("false && false;")).toBe("false");
});

test("logical OR ||", () => {
  expect(runCode("true || true;")).toBe("true");
  expect(runCode("true || false;")).toBe("true");
  expect(runCode("false || true;")).toBe("true");
  expect(runCode("false || false;")).toBe("false");
});

test("logical NOT !", () => {
  expect(runCode("!true;")).toBe("false");
  expect(runCode("!false;")).toBe("true");
  expect(runCode("!!true;")).toBe("true");
  expect(runCode("!!false;")).toBe("false");
});

test("logical AND short-circuit returns value", () => {
  expect(runCode("1 && 2;")).toBe("2");
  expect(runCode("0 && 2;")).toBe("0");
  expect(runCode("'' && 'x';")).toBe("");
  expect(runCode("'a' && 'b';")).toBe("b");
});

test("logical OR short-circuit returns value", () => {
  expect(runCode("1 || 2;")).toBe("1");
  expect(runCode("0 || 2;")).toBe("2");
  expect(runCode("'' || 'x';")).toBe("x");
  expect(runCode("null || 'default';")).toBe("default");
});

test("logical operators do not evaluate a short-circuited right operand", () => {
  expect(runCode("let n = 0; true || (n = 1); n;" )).toBe("0");
  expect(runCode("let n = 0; false && (n = 1); n;" )).toBe("0");
  expect(runCode("let n = 0; 1 ?? (n = 1); n;" )).toBe("0");
});

test("truthiness", () => {
  expect(runCode("!!0;")).toBe("false");
  expect(runCode("!!1;")).toBe("true");
  expect(runCode("!!'';")).toBe("false");
  expect(runCode("!!'hello';")).toBe("true");
  expect(runCode("!!null;")).toBe("false");
  expect(runCode("!!undefined;")).toBe("false");
  expect(runCode("!![];")).toBe("true");
  expect(runCode("!!{};")).toBe("true");
});

test("prefix increment", () => {
  expect(runCode("let i = 0; ++i;")).toBe("1");
  expect(runCode("let i = 5; ++i;")).toBe("6");
});

test("postfix increment", () => {
  expect(runCode("let i = 0; i++;")).toBe("0");
  expect(runCode("let i = 0; i++; i;")).toBe("1");
});

test("prefix decrement", () => {
  expect(runCode("let i = 5; --i;")).toBe("4");
});

test("postfix decrement", () => {
  expect(runCode("let i = 5; i--;")).toBe("5");
  expect(runCode("let i = 5; i--; i;")).toBe("4");
});

test("increment in expression", () => {
  expect(runCode("let i = 0; let j = ++i + 1; j;")).toBe("2");
  expect(runCode("let i = 0; let j = i++ + 1; j;")).toBe("1");
});

test("in operator", () => {
  expect(runCode("'a' in {a: 1, b: 2};")).toBe("true");
  expect(runCode("'c' in {a: 1, b: 2};")).toBe("false");
});

test("instanceof returns false (stub)", () => {
  expect(runCode("5 instanceof Object;")).toBe("false");
});

test("chained comparisons via &&", () => {
  expect(runCode("const x = 5; x > 3 && x < 10;")).toBe("true");
  expect(runCode("const x = 15; x > 3 && x < 10;")).toBe("false");
});

test("complex logical expressions", () => {
  expect(runCode("(true || false) && (true || false);")).toBe("true");
  expect(runCode("(false || false) && true;")).toBe("false");
  expect(runCode("!(false || false);")).toBe("true");
});
