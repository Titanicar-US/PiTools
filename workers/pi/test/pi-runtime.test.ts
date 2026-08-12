import test from "node:test";
import assert from "node:assert/strict";

import { PROTOCOL_VERSION } from "../src/protocol.js";
import {
  DiagnosisOnlyPiRuntime,
  PiSdkRuntime,
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

test("provider runtime is toolless, redacts its proposal, and always requires approval", async () => {
  let prompt = "";
  let disposed = false;
  let options: Record<string, unknown> | undefined;
  const runtime = new PiSdkRuntime(async (receivedOptions) => {
    options = receivedOptions as Record<string, unknown>;
    return {
      session: {
        messages: [
          {
            role: "assistant",
            content: JSON.stringify({
              diagnosis: "provider found OPENAI_API_KEY=sk-test-secret",
              confidence: 0.75,
              proposedFiles: ["src/main.ts"],
              proposedPatches: [{ path: "src/main.ts", unifiedDiff: "@@ -1 +1 @@\n-old\n+new\n" }],
              validationCommands: ["npm test"],
              risks: ["review required"],
              requiresApproval: false,
            }),
          },
        ],
        prompt: async (message: string) => {
          prompt = message;
        },
        waitForIdle: async () => {},
        dispose: () => {
          disposed = true;
        },
      },
    };
  });

  const result = await executePiJob(request, runtime);

  assert.equal(options?.noTools, "all");
  assert.deepEqual(options?.tools, []);
  assert.match(prompt, /Allowed paths: src\/main\.ts/);
  assert.match(prompt, /Failure evidence: npm test failed/);
  assert.doesNotMatch(prompt, /sk-test-secret/);
  assert.match(result.diagnosis, /\[REDACTED\]/);
  assert.equal(result.requiresApproval, true);
  assert.equal(disposed, true);
});
