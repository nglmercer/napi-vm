import { test, expect } from "bun:test";

import {
  missingFileSystem,
  portableCrypto,
  portablePlatform,
  posixPath,
  type HostFileSystem,
} from "../../plugins";

// ---------------------------------------------------------------------------
// Portable platform: pure POSIX paths, WebCrypto randomness, explicit wiring.
// These pin the guest-visible `napi:path` behavior (previously `node:path`),
// so any drift from Node's posix semantics fails here, not in a plugin.
// ---------------------------------------------------------------------------

test("normalize resolves segments like node posix", () => {
  expect(posixPath.normalize("")).toBe(".");
  expect(posixPath.normalize("a/b/../c")).toBe("a/c");
  expect(posixPath.normalize("/a/../../b")).toBe("/b");
  expect(posixPath.normalize("a/")).toBe("a/");
  expect(posixPath.normalize("../a")).toBe("../a");
  expect(posixPath.normalize("a//b")).toBe("a/b");
});

test("join skips empty segments", () => {
  expect(posixPath.join()).toBe(".");
  expect(posixPath.join("", "a")).toBe("a");
  expect(posixPath.join("a", "")).toBe("a");
  expect(posixPath.join("/a", "b/")).toBe("/a/b/");
  expect(posixPath.join("a", "..", "b")).toBe("b");
});

test("resolve walks right-to-left to the first absolute part", () => {
  expect(posixPath.resolve("/a", "b")).toBe("/a/b");
  expect(posixPath.resolve("a", "/b")).toBe("/b");
  expect(posixPath.resolve("..", "a")).toBe("/a");
});

test("dirname and basename handle edges", () => {
  expect(posixPath.dirname("/a/b/c")).toBe("/a/b");
  expect(posixPath.dirname("a")).toBe(".");
  expect(posixPath.dirname("/")).toBe("/");
  expect(posixPath.basename("/a/b/")).toBe("b");
  expect(posixPath.basename("a", "a")).toBe("");
  expect(posixPath.basename("a/bb", "b")).toBe("b");
});

test("extname ignores bare dots", () => {
  expect(posixPath.extname("a.tar.gz")).toBe(".gz");
  expect(posixPath.extname(".bashrc")).toBe("");
  expect(posixPath.extname("..")).toBe("");
  expect(posixPath.extname("...")).toBe(".");
});

test("relative expresses the walk between absolutes", () => {
  expect(posixPath.relative("/a/b", "/a/b")).toBe("");
  expect(posixPath.relative("/a/b", "/a/c")).toBe("../c");
  expect(posixPath.relative("/a", "/a/b/c")).toBe("b/c");
  expect(posixPath.relative("/a/b/c", "/a")).toBe("../..");
});

test("portableCrypto issues UUIDs and randomness, not digests", () => {
  const crypto = portableCrypto();
  expect(crypto.randomUUID()).toMatch(
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
  expect(crypto.randomBytes(16)).toHaveLength(16);
  expect(() => crypto.digest("sha256", new Uint8Array([1]))).toThrow(/not available/);
});

test("a missing filesystem fails with a wiring error", () => {
  const fs = missingFileSystem();
  expect(() => fs.realpath("/x")).toThrow(/no filesystem/);
  expect(() => fs.exists("/x")).toThrow(/no filesystem/);
});

test("portablePlatform assembles portable defaults around a custom fs", () => {
  const fs: HostFileSystem = {
    realpath: () => null,
    readText: () => "",
    writeText: () => {},
    exists: () => false,
  };
  const platform = portablePlatform(fs);
  expect(platform.fs).toBe(fs);
  expect(platform.path.sep).toBe("/");
  expect(typeof platform.crypto.randomUUID()).toBe("string");
  expect(platform.requireNative).toBeUndefined();
});
