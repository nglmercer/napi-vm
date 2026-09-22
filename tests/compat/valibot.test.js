import { test, expect } from "bun:test";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { assertDifferentialMatch } from "./compare.mjs";
import { runDifferentialFixture } from "./run.mjs";

const testDir = fileURLToPath(new URL(".", import.meta.url));

test("Valibot ESM schemas and transformations match Node and Bun", async () => {
  const fixture = resolve(testDir, "fixtures/npm/valibot/basic.mjs");
  const results = await runDifferentialFixture(fixture);

  assertDifferentialMatch(results, "Valibot / object / safeParse");
  expect(results.node.value).toEqual({
    objectType: "function",
    primitive: "hello",
    failed: false,
    issueCount: 1,
    parsed: true,
    output: { name: "Ada", age: 37 },
    nested: true,
    nestedOutput: {
      tags: ["portable", "js"],
      pair: ["age", 37],
      scores: { schema: 1, runtime: 2 },
      note: null,
      choice: "ready",
      event: { kind: "user", name: "Ada" },
    },
    transformed: true,
    transformedOutput: "ADA",
  });
});
