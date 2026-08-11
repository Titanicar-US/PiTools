import test from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_MAX_OUTPUT_BYTES,
  PROTOCOL_VERSION,
  validateProtocolRequest,
  validateProtocolResult,
} from "../src/protocol.js";

const request = {
  protocolVersion: PROTOCOL_VERSION,
  jobId: "job-1",
  repository: "example/repo",
  snapshotPath: "snapshot.json",
  snapshotFiles: [{ path: "src/main.rs", content: "fn main() {}\n" }],
  allowedPaths: ["src/main.rs"],
  failureEvidence: "cargo test failed",
  policyRevision: "sha256:policy",
  nonce: "nonce-1",
};

test("accepts a result bound to the request", () => {
  const result = validateProtocolResult(
    {
      protocolVersion: PROTOCOL_VERSION,
      jobId: "job-1",
      nonce: "nonce-1",
      diagnosis: "test failure",
      confidence: 0.8,
      proposedFiles: ["src/main.rs"],
      validationCommands: ["cargo test"],
      risks: [],
      requiresApproval: true,
    },
    request,
  );

  assert.equal(result.jobId, "job-1");
});

test("rejects an unbound result", () => {
  assert.throws(() =>
    validateProtocolResult(
      {
        protocolVersion: PROTOCOL_VERSION,
        jobId: "other-job",
        nonce: "nonce-1",
        diagnosis: "test failure",
        confidence: 0.8,
        proposedFiles: [],
        validationCommands: [],
        risks: [],
        requiresApproval: true,
      },
      request,
    ),
  );
});

test("rejects a result with a different protocol version", () => {
  assert.throws(
    () =>
      validateProtocolResult(
        {
          protocolVersion: "pitools.pi/v2",
          jobId: "job-1",
          nonce: "nonce-1",
          diagnosis: "test failure",
          confidence: 0.8,
          proposedFiles: [],
          validationCommands: [],
          risks: [],
          requiresApproval: true,
        },
        request,
      ),
    /invalid PiTools result|not bound to the request/,
  );
});

test("rejects unknown request and result fields", () => {
  assert.throws(
    () => validateProtocolRequest({ ...request, githubToken: "not-allowed" }),
    /invalid PiTools request/,
  );

  assert.throws(
    () =>
      validateProtocolResult(
        {
          protocolVersion: PROTOCOL_VERSION,
          jobId: "job-1",
          nonce: "nonce-1",
          diagnosis: "test failure",
          confidence: 0.8,
          proposedFiles: [],
          validationCommands: [],
          risks: [],
          requiresApproval: true,
          unexpected: true,
        },
        request,
      ),
    /invalid PiTools result/,
  );
});

test("rejects non-canonical snapshot paths at the worker boundary", () => {
  assert.throws(
    () => validateProtocolRequest({ ...request, snapshotPath: "/workspace/repo" }),
    /canonical repository-relative path/,
  );
  assert.throws(
    () => validateProtocolRequest({ ...request, snapshotPath: "../repo" }),
    /canonical repository-relative path/,
  );
});

test("rejects invalid confidence values", () => {
  assert.throws(() =>
    validateProtocolResult(
      {
        protocolVersion: PROTOCOL_VERSION,
        jobId: "job-1",
        nonce: "nonce-1",
        diagnosis: "test failure",
        confidence: 1.1,
        proposedFiles: [],
        validationCommands: [],
        risks: [],
        requiresApproval: true,
      },
      request,
    ),
  );
});

test("enforces the serialized UTF-8 output limit", () => {
  const value = {
    protocolVersion: PROTOCOL_VERSION,
    jobId: "job-1",
    nonce: "nonce-1",
    diagnosis: "é".repeat(20),
    confidence: 0.8,
    proposedFiles: [],
    validationCommands: [],
    risks: [],
    requiresApproval: true,
  };
  const asciiBytes = Buffer.byteLength(JSON.stringify({ ...value, diagnosis: "e".repeat(20) }));

  assert.throws(
    () => validateProtocolResult(value, request, { maxOutputBytes: asciiBytes }),
    new RegExp(`exceeds the ${asciiBytes} byte limit`),
  );
  assert.equal(DEFAULT_MAX_OUTPUT_BYTES, 64 * 1024);
});

test("validates bounded patch proposals against the request allowlist", () => {
  const result = {
    protocolVersion: PROTOCOL_VERSION,
    jobId: "job-1",
    nonce: "nonce-1",
    diagnosis: "test failure",
    confidence: 0.8,
    proposedFiles: ["src/main.rs"],
    proposedPatches: [
      {
        path: "src/main.rs",
        unifiedDiff: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n",
      },
    ],
    validationCommands: ["cargo test"],
    risks: [],
    requiresApproval: true,
  };

  assert.doesNotThrow(() => validateProtocolResult(result, request));
  assert.throws(
    () =>
      validateProtocolResult(
        {
          ...result,
          proposedPatches: [{ ...result.proposedPatches[0], path: "src/other.ts" }],
        },
        request,
      ),
    /outside the allowlist/,
  );
});

test("rejects typed patches that do not require explicit approval", () => {
  const result = {
    protocolVersion: PROTOCOL_VERSION,
    jobId: "job-1",
    nonce: "nonce-1",
    diagnosis: "test failure",
    confidence: 0.8,
    proposedFiles: ["src/main.rs"],
    proposedPatches: [
      {
        path: "src/main.rs",
        unifiedDiff: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n",
      },
    ],
    validationCommands: ["cargo test"],
    risks: [],
    requiresApproval: false,
  };

  assert.throws(
    () => validateProtocolResult(result, request),
    /must require explicit approval/,
  );
});
