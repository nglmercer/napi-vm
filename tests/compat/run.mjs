import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const runnerDir = resolve(root, "tests/compat/runners");

const statusFor = (runtime, code, stderr, error) => {
  if (error?.code === "ETIMEDOUT") return "timeout";
  if (code === 0) return "pass";
  if (/SyntaxError|Unexpected token|ParseError/i.test(stderr)) {
    return runtime === "napi-vm" ? "parse-error" : "compile-error";
  }
  return "runtime-error";
};

function execute(runtime, command, args, timeout) {
  return new Promise((resolveResult) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeout);

    child.stdout.setEncoding("utf8").on("data", (chunk) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      resolveResult({
        runtime,
        status: "runtime-error",
        stdout: [],
        error: { name: error.name, message: error.message },
      });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      const lines = stdout.replace(/\r\n/g, "\n").trim().split("\n").filter(Boolean);
      const status = timedOut ? "timeout" : statusFor(runtime, code, stderr);
      if (status !== "pass") {
        resolveResult({
          runtime,
          status,
          stdout: lines,
          error: { message: stderr.trim() || `process exited with code ${code}` },
        });
        return;
      }

      try {
        const value = JSON.parse(lines.pop() ?? "");
        resolveResult({ runtime, status, stdout: lines, value });
      } catch (error) {
        resolveResult({
          runtime,
          status: "runtime-error",
          stdout: lines,
          error: { name: error.name, message: `runner output was not JSON: ${error.message}` },
        });
      }
    });
  });
}

/** Execute one unchanged fixture under Node, Bun, and napi-vm. */
export async function runDifferentialFixture(fixture, { timeout = 5000 } = {}) {
  const path = resolve(fixture);
  const runs = await Promise.all([
    execute("node", "node", [resolve(runnerDir, "node.mjs"), path], timeout),
    execute("bun", "bun", [resolve(runnerDir, "bun.mjs"), path], timeout),
    execute("napi-vm", "bun", [resolve(runnerDir, "napi-vm.mjs"), path], timeout),
  ]);
  return Object.fromEntries(runs.map((result) => [result.runtime, result]));
}
