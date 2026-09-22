import { deepStrictEqual } from "node:assert";

const failureCategories = [
  "MODULE_RESOLUTION",
  "UNSUPPORTED_SYNTAX",
  "SWC_TRANSFORM",
  "MISSING_BUILTIN",
  "MISSING_METHOD",
  "WRONG_SEMANTICS",
  "EVENT_LOOP_ORDER",
  "PROMISE_SEMANTICS",
  "ERROR_SEMANTICS",
  "WEB_API",
  "HOST_BRIDGE",
  "SANDBOX_POLICY",
  "NATIVE_DEPENDENCY",
  "UNKNOWN",
];

function classifyResultFailure(result) {
  if (result.status === "parse-error" || result.status === "compile-error") {
    return "UNSUPPORTED_SYNTAX";
  }
  if (result.status === "timeout") return "UNKNOWN";
  const message = result.error?.message ?? "";
  if (/module not found|cannot find module|failed to resolve/i.test(message)) {
    return "MODULE_RESOLUTION";
  }
  if (/not defined|is not a function/i.test(message)) return "MISSING_BUILTIN";
  return "UNKNOWN";
}

/** Compare structured napi-vm results with both reference runtimes. */
export function assertDifferentialMatch(results, label) {
  for (const runtime of ["node", "bun", "napi-vm"]) {
    const result = results[runtime];
    if (result.status !== "pass") {
      const category = failureCategories.includes(result.category)
        ? result.category
        : classifyResultFailure(result);
      throw new Error(
        `[${category}] ${label} failed in ${runtime}: ${JSON.stringify(result)}`,
      );
    }
  }

  try {
    deepStrictEqual(results.bun.value, results.node.value);
    deepStrictEqual(results["napi-vm"].value, results.node.value);
  } catch (error) {
    throw new Error(`[WRONG_SEMANTICS] ${label}: ${error.message}`);
  }
}
