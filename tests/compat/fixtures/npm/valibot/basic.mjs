import * as v from "valibot";

const Primitive = v.string();
const primitive = v.parse(Primitive, "hello");
const failed = v.safeParse(Primitive, 42);
const User = v.object({
  name: v.string(),
  age: v.number(),
});
const parsed = v.safeParse(User, {
  name: "Ada",
  age: 37,
});
const Nested = v.object({
  tags: v.array(v.string()),
  pair: v.tuple([v.string(), v.number()]),
  scores: v.record(v.string(), v.number()),
  note: v.optional(v.nullable(v.string())),
  choice: v.union([v.literal("ready"), v.number()]),
  event: v.variant("kind", [
    v.object({ kind: v.literal("user"), name: v.string() }),
    v.object({ kind: v.literal("system"), code: v.number() }),
  ]),
});
const nested = v.safeParse(Nested, {
  tags: ["portable", "js"],
  pair: ["age", 37],
  scores: { schema: 1, runtime: 2 },
  note: null,
  choice: "ready",
  event: { kind: "user", name: "Ada" },
});
const Transformed = v.pipe(
  v.string(),
  v.trim(),
  v.toUpperCase(),
  v.minLength(2),
  v.maxLength(8),
  v.regex(/^[A-Z]+$/u),
  v.check((value) => value !== "INVALID", "value is reserved"),
);
const transformed = v.safeParse(Transformed, " ada ");

console.log(JSON.stringify({
  objectType: typeof v.object,
  primitive,
  failed: failed.success,
  issueCount: failed.issues.length,
  parsed: parsed.success,
  output: parsed.output,
  nested: nested.success,
  nestedOutput: nested.output,
  transformed: transformed.success,
  transformedOutput: transformed.output,
}));
