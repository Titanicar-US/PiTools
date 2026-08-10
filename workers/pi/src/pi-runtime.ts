import { createAgentSession } from "@earendil-works/pi-coding-agent";
import {
  type PiJobRequest,
  type PiJobResult,
  type ProtocolLimits,
  validateProtocolRequest,
  validateProtocolResult,
} from "./protocol.js";

import { redactSecrets } from "./redaction.js";

export interface PiRuntimeAdapter {
  execute(request: PiJobRequest): Promise<unknown>;
}

export class DiagnosisOnlyPiRuntime implements PiRuntimeAdapter {
  async execute(request: PiJobRequest): Promise<unknown> {
    return {
      protocolVersion: request.protocolVersion,
      jobId: request.jobId,
      nonce: request.nonce,
      diagnosis: request.failureEvidence ?? "No failure evidence supplied",
      confidence: 0,
      proposedFiles: [],
      proposedPatches: [],
      validationCommands: [],
      risks: ["No Pi workflow was selected; no mutation is proposed"],
      requiresApproval: true,
    };
  }
}

/**
 * Optional provider-backed Pi runtime. It is deliberately toolless: the Rust
 * control plane owns repository mutations and validation, while Pi only
 * returns a bounded diagnosis/proposal for explicit approval.
 */
export class PiSdkRuntime implements PiRuntimeAdapter {
  async execute(request: PiJobRequest): Promise<unknown> {
    const agentDir = process.env.PITOOLS_PI_AGENT_DIR;
    const { session } = await createAgentSession({
      cwd: process.cwd(),
      noTools: "all",
      tools: [],
      ...(agentDir === undefined ? {} : { agentDir }),
    });
    try {
      await session.prompt(
        [
          "You are the PiTools diagnosis worker.",
          "Return exactly one JSON object with keys diagnosis, confidence, proposedFiles, proposedPatches, validationCommands, risks, requiresApproval.",
          "Do not claim that files were changed. Do not include credentials or raw secrets.",
          `Repository: ${request.repository}`,
          `Allowed paths: ${request.allowedPaths.join(", ")}`,
          `Snapshot files:\n${request.snapshotFiles.length === 0
            ? "not supplied"
            : request.snapshotFiles
                .map((file) => `--- ${file.path} ---\n${file.content}`)
                .join("\n")}`,
          `Failure evidence: ${request.failureEvidence ?? "not supplied"}`,
        ].join("\n"),
      );
      await session.waitForIdle();
      const raw = assistantText(session.messages);
      const parsed = parseProposal(raw);
      return {
        protocolVersion: request.protocolVersion,
        jobId: request.jobId,
        nonce: request.nonce,
        diagnosis: parsed.diagnosis,
        confidence: parsed.confidence,
        proposedFiles: parsed.proposedFiles,
        proposedPatches: parsed.proposedPatches,
        validationCommands: parsed.validationCommands,
        risks: parsed.risks,
        requiresApproval: true,
      };
    } finally {
      session.dispose();
    }
  }
}

type Proposal = {
  diagnosis: string;
  confidence: number;
  proposedFiles: string[];
  proposedPatches: Array<{ path: string; unifiedDiff: string }>;
  validationCommands: string[];
  risks: string[];
};

function assistantText(messages: readonly unknown[]): string {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (!isRecord(message) || message.role !== "assistant") continue;
    const content = message.content;
    if (typeof content === "string") return content;
    if (!Array.isArray(content)) continue;
    const text = content
      .filter(isRecord)
      .filter((part) => part.type === "text" && typeof part.text === "string")
      .map((part) => part.text as string)
      .join("\n");
    if (text) return text;
  }
  return "Pi returned no assistant diagnosis";
}

function parseProposal(raw: string): Proposal {
  const redacted = String(redactSecrets(raw));
  try {
    const candidate: unknown = JSON.parse(redacted);
    if (!isRecord(candidate)) throw new Error("proposal is not an object");
    const diagnosis = typeof candidate.diagnosis === "string" ? candidate.diagnosis : redacted;
    const confidence = typeof candidate.confidence === "number" && Number.isFinite(candidate.confidence)
      ? Math.max(0, Math.min(1, candidate.confidence))
      : 0;
    const proposedFiles = stringArray(candidate.proposedFiles);
    const proposedPatches = patchArray(candidate.proposedPatches);
    const validationCommands = stringArray(candidate.validationCommands);
    const risks = stringArray(candidate.risks);
    return { diagnosis, confidence, proposedFiles, proposedPatches, validationCommands, risks };
  } catch {
    return {
      diagnosis: redacted,
      confidence: 0,
      proposedFiles: [],
      proposedPatches: [],
      validationCommands: [],
      risks: ["Pi response was not valid proposal JSON"],
    };
  }
}

function patchArray(value: unknown): Array<{ path: string; unifiedDiff: string }> {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!isRecord(item) || typeof item.path !== "string" || typeof item.unifiedDiff !== "string") {
      return [];
    }
    return [{ path: item.path, unifiedDiff: item.unifiedDiff }];
  });
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export async function executePiJob(
  value: unknown,
  runtime: PiRuntimeAdapter,
  limits: ProtocolLimits = {},
): Promise<PiJobResult> {
  const request = validateProtocolRequest(value);
  const result = await runtime.execute(request);
  return validateProtocolResult(result, request, limits);
}
