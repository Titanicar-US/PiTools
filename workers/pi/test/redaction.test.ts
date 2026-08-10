import test from "node:test";
import assert from "node:assert/strict";

import { PROTOCOL_VERSION, validateProtocolResult } from "../src/protocol.js";
import { redactSecrets } from "../src/redaction.js";

test("redacts secret-like output before it crosses the worker boundary", () => {
  const request = {
    protocolVersion: PROTOCOL_VERSION,
    jobId: "job-1",
    repository: "example/repo",
    snapshotPath: "snapshot.json",
    snapshotFiles: [],
    allowedPaths: [],
    policyRevision: "sha256:policy",
    nonce: "nonce-1",
  };

  const result = validateProtocolResult(
    {
        protocolVersion: PROTOCOL_VERSION,
        jobId: "job-1",
        nonce: "nonce-1",
        diagnosis: "token ghp_1234567890 leaked; key sk-abcdefghijklmnop",
        confidence: 0.1,
        proposedFiles: [],
        validationCommands: [],
        risks: ["Authorization: Bearer abc.def-123"],
        requiresApproval: true,
      },
    request,
  );

  assert.equal(result.diagnosis, "token [REDACTED] leaked; key [REDACTED]");
  assert.deepEqual(result.risks, ["Authorization: [REDACTED]"]);
});

test("recursively redacts sensitive metadata keys and nested values", () => {
  const redacted = redactSecrets({
    apiKey: "plain-secret-value",
    nested: [{ output: "github_pat_abcdefghijklmnopqrstuvwxyz" }],
    safe: "visible",
  });

  assert.deepEqual(redacted, {
    apiKey: "[REDACTED]",
    nested: [{ output: "[REDACTED]" }],
    safe: "visible",
  });
});

test("redacts secret values from errors", () => {
  const redacted = redactSecrets(new Error("provider used sk-abcdefghijklmnop"));

  assert.equal(redacted, "Error: provider used [REDACTED]");
});

test("redacts extended provider, cloud, package, JWT, and PEM secrets", () => {
  const values = [
    "ghs_1234567890",
    "gho_1234567890",
    "AKIA1234567890ABCDEF",
    "npm_abcdefghijklmnop",
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.signature",
    "-----BEGIN PRIVATE KEY-----\nprivate-bytes\n-----END PRIVATE KEY-----",
    "TOKEN=plain-secret",
  ];
  const redacted = String(redactSecrets(values));
  for (const value of values) {
    assert.equal(redacted.includes(value), false, `secret leaked: ${value}`);
  }
});
