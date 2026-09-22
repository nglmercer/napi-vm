import { afterAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Vm } from "../index.js";
import { GuestPackageLoader, IdentityCompiler, NpmCompatibilityError, scanModuleSource } from "../plugins/npm/index.ts";
import { nodePlatform } from "../plugins/node.ts";

const tempRoots = [];

function makePackageTree(files) {
  const root = mkdtempSync(join(tmpdir(), "napi-vm-npm-"));
  tempRoots.push(root);
  for (const [relative, source] of Object.entries(files)) {
    const filename = join(root, relative);
    mkdirSync(dirname(filename), { recursive: true });
    writeFileSync(filename, source);
  }
  return root;
}

function standardTree() {
  return makePackageTree({
    "node_modules/@fixture/root/package.json": JSON.stringify({
      name: "@fixture/root",
      version: "1.2.3",
      type: "module",
      exports: {
        ".": {
          import: "./src/index.mjs",
          require: "./dist/require.cjs",
          default: "./src/default.mjs",
        },
        "./feature/*": {
          import: "./src/features/*.mjs",
        },
      },
    }),
    "node_modules/@fixture/root/src/index.mjs": `
      globalThis.guestLoaded = true;
      import { shared } from "./shared";
      import { dep } from "@fixture/dep";
      export const value = shared + dep;
      export default value;
    `,
    "node_modules/@fixture/root/src/shared.mjs": "export const shared = 40;",
    "node_modules/@fixture/root/src/features/extra.mjs": "export const feature = 16;",
    "node_modules/@fixture/root/node_modules/@fixture/dep/package.json": JSON.stringify({
      name: "@fixture/dep",
      version: "2.0.0",
      exports: { ".": { import: "./index.mjs", default: "./other.mjs" } },
    }),
    "node_modules/@fixture/root/node_modules/@fixture/dep/index.mjs": "export const dep = 2;",
    "node_modules/@fixture/root/node_modules/@fixture/dep/other.mjs": "export const dep = 1000;",
    "node_modules/@fixture/root/dist/require.cjs": "module.exports = {};",
    "node_modules/@fixture/root/src/default.mjs": "export const value = 1000;",
  });
}

afterAll(() => {
  for (const root of tempRoots) rmSync(root, { recursive: true, force: true });
});

test("package loader resolves conditional exports and registers guest ESM without host execution", async () => {
  const rootDir = standardTree();
  const vm = new Vm();
  try {
    const loader = new GuestPackageLoader(vm, { platform: nodePlatform(), rootDir });
    const entry = await loader.loadPackage("@fixture/root");
    await loader.loadPackage("@fixture/root/feature/extra");

    expect(entry).toBe("/npm/@fixture/root@1.2.3/src/index.mjs");
    expect(vm.hasModule(entry)).toBe(true);
    expect(vm.run("typeof guestLoaded;")).toBe("undefined");
    expect(
      vm.run(`
        import defaultValue, { value } from "@fixture/root";
        import { feature } from "@fixture/root/feature/extra";
        defaultValue + value + feature;
      `),
    ).toBe("100");
    expect(vm.run("guestLoaded;")).toBe("true");
    expect(globalThis.guestLoaded).toBeUndefined();
  } finally {
    vm.dispose();
  }
});

test("compiler mode none keeps JavaScript unchanged and reports unsupported source", async () => {
  const rootDir = makePackageTree({
    "node_modules/typed-pkg/package.json": JSON.stringify({ name: "typed-pkg", version: "1.0.0", main: "index.ts" }),
    "node_modules/typed-pkg/index.ts": "export const value: number = 42;",
  });
  const vm = new Vm();
  try {
    const identity = new IdentityCompiler();
    expect(await identity.compile({ filename: "a.js", source: "export const a = 1;", syntax: "js" })).toEqual({
      code: "export const a = 1;",
    });
    const loader = new GuestPackageLoader(vm, { platform: nodePlatform(), rootDir, compilerMode: "none" });
    await expect(loader.loadPackage("typed-pkg")).rejects.toMatchObject({
      name: "NpmCompatibilityError",
      category: "UNSUPPORTED_SYNTAX",
    });
  } finally {
    vm.dispose();
  }
});

test("auto compiler fallback is followed by napi-vm validation", async () => {
  const rootDir = makePackageTree({
    "node_modules/typed-pkg/package.json": JSON.stringify({ name: "typed-pkg", version: "1.0.0", main: "index.ts" }),
    "node_modules/typed-pkg/index.ts": "export const value: number = 5;",
  });
  const vm = new Vm();
  const calls = [];
  try {
    const loader = new GuestPackageLoader(vm, {
      platform: nodePlatform(),
      rootDir,
      compilerMode: "auto",
      compiler: {
        async compile(input) {
          calls.push(input);
          return { code: "export const value = 42;" };
        },
      },
    });
    const entry = await loader.loadPackage("typed-pkg");
    expect(calls[0].syntax).toBe("ts");
    expect(vm.run(`import { value } from "${entry}"; value;`)).toBe("42");
  } finally {
    vm.dispose();
  }
});

test("SWC output that the VM cannot parse is rejected as a transform failure", async () => {
  const rootDir = makePackageTree({
    "node_modules/typed-pkg/package.json": JSON.stringify({ name: "typed-pkg", version: "1.0.0", main: "index.ts" }),
    "node_modules/typed-pkg/index.ts": "export const value: number = 5;",
  });
  const vm = new Vm();
  try {
    const loader = new GuestPackageLoader(vm, {
      platform: nodePlatform(),
      rootDir,
      compilerMode: "auto",
      compiler: { async compile() { return { code: "export const = ;" }; } },
    });
    await expect(loader.loadPackage("typed-pkg")).rejects.toMatchObject({
      name: "NpmCompatibilityError",
      category: "SWC_TRANSFORM",
    });
  } finally {
    vm.dispose();
  }
});

test("CommonJS package source is rejected instead of interpreted as guest ESM", async () => {
  const rootDir = makePackageTree({
    "node_modules/commonjs-pkg/package.json": JSON.stringify({ name: "commonjs-pkg", version: "1.0.0", main: "index.js" }),
    "node_modules/commonjs-pkg/index.js": "module.exports = require('./hidden.js');",
    "node_modules/commonjs-pkg/hidden.js": "globalThis.hostMustNotRun = true;",
  });
  const vm = new Vm();
  try {
    const loader = new GuestPackageLoader(vm, { platform: nodePlatform(), rootDir });
    await expect(loader.loadPackage("commonjs-pkg")).rejects.toMatchObject({
      name: "NpmCompatibilityError",
      category: "MODULE_RESOLUTION",
    });
    expect(vm.listModules()).toEqual([]);
    expect(globalThis.hostMustNotRun).toBeUndefined();
  } finally {
    vm.dispose();
  }
});

test("module graph scanner ignores comments and string literals", () => {
  const result = scanModuleSource(`
    // import "ignored-comment";
    const message = "export * from 'ignored-string'";
    import { value } from "./value.mjs";
    export { named as alias } from "./named.mjs";
    export { source as default } from "./default.mjs";
  `);
  expect(result.imports.map(({ specifier }) => specifier)).toEqual([
    "./value.mjs",
    "./named.mjs",
    "./default.mjs",
  ]);
  expect(result.hasDefaultExport).toBe(true);
  expect(result.hasCommonJs).toBe(false);
  expect(result.hasNonLiteralDynamicImport).toBe(false);
});

test("module graph scanner rejects computed dynamic imports and CommonJS patterns", () => {
  expect(scanModuleSource("export const load = (name) => import(name);").hasNonLiteralDynamicImport).toBe(true);
  expect(scanModuleSource("module.exports = require('./legacy.cjs');").hasCommonJs).toBe(true);
});
