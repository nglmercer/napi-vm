import { test, expect } from "bun:test";

import { validateEntryPath } from "../../plugins";

// ---------------------------------------------------------------------------
// Manifest entry paths: validated and normalized here so `manifest.ts`
// stays agnostic to paths. The host calls this after parsing; the canonical
// containment check still happens on the real path at load.
// ---------------------------------------------------------------------------

test("rejects an entry outside the plugin directory", () => {
  expect(() => validateEntryPath("../../../outside.js")).toThrow(
    /inside the plugin directory/,
  );
  expect(() => validateEntryPath("/etc/passwd")).toThrow(
    /inside the plugin directory/,
  );
});

test("normalizes the entry path", () => {
  expect(validateEntryPath("./src/../plugin.js")).toBe("plugin.js");
  expect(validateEntryPath("src\\main.js")).toBe("src/main.js");
});

test("rejects an empty entry", () => {
  expect(() => validateEntryPath("")).toThrow(/entry must be a non-empty string/);
  expect(() => validateEntryPath("   ")).toThrow(/entry must be a non-empty string/);
});

test("an entry whose first segment merely starts with `..` is accepted", () => {
  expect(validateEntryPath("./..build/plugin.js")).toBe("..build/plugin.js");
});

test("an entry with a real `..` segment is still rejected", () => {
  for (const entry of ["../plugin.js", "..", "a/../../plugin.js"]) {
    expect(() => validateEntryPath(entry)).toThrow(/inside the plugin directory/);
  }
});
