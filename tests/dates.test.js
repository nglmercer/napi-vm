import { test, expect } from "bun:test";
import { runCode } from "../index.js";

// ---------------------------------------------------------------------------
// `Date` instances. The sandbox has no local timezone, so every accessor is
// UTC and `getTimezoneOffset()` is zero.
// ---------------------------------------------------------------------------

test("typeof Date is function", () => {
  expect(runCode("typeof Date;")).toBe("function");
});

test("epoch milliseconds construct a date", () => {
  expect(runCode("new Date(0).toISOString();")).toBe("1970-01-01T00:00:00.000Z");
});

test("an ISO string constructs a date", () => {
  expect(runCode("new Date('2024-01-15T10:30:00Z').getFullYear();")).toBe("2024");
});

test("components construct a date", () => {
  expect(runCode("new Date(2024, 0, 15).toISOString();")).toBe("2024-01-15T00:00:00.000Z");
});

test("component accessors", () => {
  expect(runCode("new Date('2024-06-15T12:34:56.789Z').getMonth();")).toBe("5");
  expect(runCode("new Date('2024-06-15T12:34:56.789Z').getDate();")).toBe("15");
  expect(runCode("new Date('2024-06-15T12:34:56.789Z').getHours();")).toBe("12");
  expect(runCode("new Date('2024-06-15T12:34:56.789Z').getMinutes();")).toBe("34");
  expect(runCode("new Date('2024-06-15T12:34:56.789Z').getSeconds();")).toBe("56");
  expect(runCode("new Date('2024-06-15T12:34:56.789Z').getMilliseconds();")).toBe("789");
});

test("the day of the week", () => {
  // 1970-01-01 was a Thursday.
  expect(runCode("new Date(0).getDay();")).toBe("4");
});

test("UTC accessors match the local ones", () => {
  expect(runCode("const d = new Date(0); d.getHours() === d.getUTCHours();")).toBe("true");
  expect(runCode("new Date(0).getTimezoneOffset();")).toBe("0");
});

test("getTime and valueOf agree", () => {
  expect(runCode("new Date(1234).getTime();")).toBe("1234");
  expect(runCode("new Date(1234).valueOf();")).toBe("1234");
});

test("setTime is observed through every reference", () => {
  expect(runCode("const d = new Date(0); const alias = d; d.setTime(1000); alias.getTime();")).toBe(
    "1000",
  );
});

test("a date stringifies as its ISO form", () => {
  expect(runCode("new Date(0) + '';")).toBe("1970-01-01T00:00:00.000Z");
});

test("a date serializes to JSON as its ISO form", () => {
  expect(runCode("JSON.stringify({ d: new Date(0) });")).toBe(
    '{"d":"1970-01-01T00:00:00.000Z"}',
  );
});

test("an invalid date reports itself", () => {
  expect(() => runCode("new Date(NaN).toISOString();")).toThrow(/RangeError/);
  expect(runCode("new Date(NaN).toString();")).toBe("Invalid Date");
  expect(runCode("new Date(NaN).toJSON();")).toBe("null");
});

test("dates before the epoch work", () => {
  expect(runCode("new Date(-86400000).toISOString();")).toBe("1969-12-31T00:00:00.000Z");
});

test("Date.now is a number", () => {
  expect(runCode("typeof Date.now();")).toBe("number");
});

test("Date without new is a string", () => {
  expect(runCode("typeof Date();")).toBe("string");
});

test("a date copies another date", () => {
  expect(runCode("new Date(new Date(77)).getTime();")).toBe("77");
});
