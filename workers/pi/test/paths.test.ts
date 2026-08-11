import test from "node:test";
import assert from "node:assert/strict";

import { assertAllowedPaths } from "../src/paths.js";

test("accepts exact canonical repository-relative paths", () => {
  assert.doesNotThrow(() => assertAllowedPaths(["src/main.ts"], ["src/main.ts"]));
});

test("rejects paths outside the exact allowlist", () => {
  assert.throws(
    () => assertAllowedPaths(["src/other.ts"], ["src/main.ts"]),
    /outside the allowlist/,
  );
});

test("rejects absolute, traversal, and non-canonical paths", () => {
  for (const candidate of [
    "/workspace/repo/src/main.ts",
    "C:\\workspace\\repo\\src\\main.ts",
    "src/../secrets.txt",
    "./src/main.ts",
    "src//main.ts",
    "src\\main.ts",
  ]) {
    assert.throws(
      () => assertAllowedPaths([candidate], [candidate]),
      /canonical repository-relative path/,
      candidate,
    );
  }
});
