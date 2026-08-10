import test from "node:test";
import assert from "node:assert/strict";

import { PROTOCOL_VERSION } from "../src/protocol.js";
import {
  DiagnosisOnlyPiRuntime,
  executePiJob,
  type PiRuntimeAdapter,
} from "../src/pi-runtime.js";

const request = {
  protocolVersion: PROTOCOL_VERSION,
  jobId: "job-1",
  repository: "example/repo",
  snapshotPath: "snapshot.json",
  snapshotFiles: [{ path: "src/main.ts", content: "export const main = true;\n" }],
  allowedPaths: ["src/main.ts"],
  failureEvidence: "npm test failed",
  policyRevision: "sha256:policy",
  nonce: "nonce-1",
};

test("executes through an injected runtime without provider credentials", async () => {
  const runtime: PiRuntimeAdapter = {
    execute: async (boundRequest) => ({
      protocolVersion: boundRequest.protocolVersion,
      jobId: boundRequest.jobId,
      nonce: boundRequest.nonce,
      diagnosis: "deterministic diagnosis",
      confidence: 0.5,
      proposedFiles: ["src/main.ts"],
      validationCommands: ["npm test"],
      risks: [],
      requiresApproval: true,
    }),
  };

  const result = await executePiJob(request, runtime);

  assert.equal(result.diagnosis, "deterministic diagnosis");
});

test("validates the runtime result against the exact request", async () => {
  const runtime: PiRuntimeAdapter = {
    execute: async () => ({
      protocolVersion: PROTOCOL_VERSION,
      jobId: "different-job",
      nonce: "nonce-1",
      diagnosis: "wrong job",
      confidence: 0.5,
      proposedFiles: [],
      validationCommands: [],
      risks: [],
      requiresApproval: true,
    }),
  };

  await assert.rejects(() => executePiJob(request, runtime), /not bound to the request/);
});

test("diagnosis-only runtime is deterministic and proposes no mutation", async () => {
  const runtime = new DiagnosisOnlyPiRuntime();

  const first = await executePiJob(request, runtime);
  const second = await executePiJob(request, runtime);

  assert.deepEqual(first, second);
  assert.deepEqual(first.proposedFiles, []);
  assert.equal(first.requiresApproval, true);
});
