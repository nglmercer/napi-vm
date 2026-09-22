import { test, expect } from "bun:test";
import { deepStrictEqual } from "node:assert";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runDifferentialFixture } from "./run.mjs";

const testDir = fileURLToPath(new URL(".", import.meta.url));

test("standard promise, microtask, and timer ordering matches Node and Bun", async () => {
  const fixture = resolve(testDir, "fixtures/event-loop/basic-order.mjs");
  const results = await runDifferentialFixture(fixture);

  for (const result of Object.values(results)) {
    expect(result.status, `${result.runtime}: ${result.error?.message ?? ""}`).toBe("pass");
    expect(result.stdout).toEqual([]);
  }

  try {
    deepStrictEqual(results.bun.value, results.node.value);
    deepStrictEqual(results["napi-vm"].value, results.node.value);
  } catch (error) {
    throw new Error(`EVENT_LOOP_ORDER: ${error.message}`);
  }
});
