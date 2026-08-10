import {
  array,
  boolean,
  finite,
  type InferOutput,
  literal,
  maxLength,
  maxValue,
  minLength,
  minValue,
  number,
  optional,
  pipe,
  record,
  safeParse,
  strictObject,
  string,
  unknown,
} from "valibot";

import { assertAllowedPaths, assertCanonicalRepositoryPath } from "./paths.js";
import { redactSecrets } from "./redaction.js";

export const PROTOCOL_VERSION = "pitools.pi/v1" as const;
export const DEFAULT_MAX_OUTPUT_BYTES = 64 * 1024;

const nonEmptyString = pipe(string(), minLength(1));
const patchPath = pipe(string(), minLength(1), maxLength(1024));
const unifiedDiff = pipe(string(), minLength(1), maxLength(128 * 1024));

const piPatchSchema = strictObject({
  path: patchPath,
  unifiedDiff,
});

export const piJobRequestSchema = strictObject({
  protocolVersion: literal(PROTOCOL_VERSION),
  jobId: nonEmptyString,
  repository: nonEmptyString,
  snapshotPath: nonEmptyString,
  allowedPaths: array(nonEmptyString),
  failureEvidence: optional(string()),
  policyRevision: nonEmptyString,
  nonce: nonEmptyString,
});

export const piJobResultSchema = strictObject({
  protocolVersion: literal(PROTOCOL_VERSION),
  jobId: nonEmptyString,
  nonce: nonEmptyString,
  diagnosis: string(),
  confidence: pipe(number(), finite(), minValue(0), maxValue(1)),
  proposedFiles: array(nonEmptyString),
  proposedPatches: optional(array(piPatchSchema)),
  validationCommands: array(nonEmptyString),
  risks: array(string()),
  requiresApproval: boolean(),
  metadata: optional(record(string(), unknown())),
});

export type PiJobRequest = InferOutput<typeof piJobRequestSchema>;
export type PiJobResult = InferOutput<typeof piJobResultSchema>;

export type ProtocolLimits = {
  maxOutputBytes?: number;
};

function parseRequest(value: unknown): PiJobRequest {
  const parsed = safeParse(piJobRequestSchema, value);
  if (!parsed.success) {
    throw new Error("invalid PiTools request");
  }
  return parsed.output;
}

function parseResult(value: unknown): PiJobResult {
  const parsed = safeParse(piJobResultSchema, value);
  if (!parsed.success) {
    throw new Error("invalid PiTools result");
  }
  return parsed.output;
}

function assertOutputSize(value: unknown, maxOutputBytes: number): void {
  if (!Number.isSafeInteger(maxOutputBytes) || maxOutputBytes < 1) {
    throw new Error("PiTools output limit must be a positive safe integer");
  }

  let serialized: string;
  try {
    const output = JSON.stringify(value);
    if (output === undefined) {
      throw new TypeError("value is not JSON serializable");
    }
    serialized = output;
  } catch {
    throw new Error("PiTools result is not JSON serializable");
  }

  if (Buffer.byteLength(serialized, "utf8") > maxOutputBytes) {
    throw new Error(`PiTools result exceeds the ${maxOutputBytes} byte limit`);
  }
}

export function validateProtocolRequest(value: unknown): PiJobRequest {
  const request = parseRequest(value);
  assertCanonicalRepositoryPath(request.snapshotPath);
  assertAllowedPaths([], request.allowedPaths);
  return request;
}

export function validateProtocolResult(
  value: unknown,
  request: PiJobRequest,
  limits: ProtocolLimits = {},
): PiJobResult {
  const maxOutputBytes = limits.maxOutputBytes ?? DEFAULT_MAX_OUTPUT_BYTES;
  assertOutputSize(value, maxOutputBytes);

  const result = parseResult(value);
  if (
    result.protocolVersion !== request.protocolVersion ||
    result.jobId !== request.jobId ||
    result.nonce !== request.nonce
  ) {
    throw new Error("PiTools result is not bound to the request");
  }
  if (result.proposedPatches && result.proposedPatches.length > 0 && !result.requiresApproval) {
    throw new Error("PiTools typed patches must require explicit approval");
  }
  assertAllowedPaths(result.proposedFiles, request.allowedPaths);
  assertAllowedPaths(
    result.proposedPatches?.map((patch) => patch.path) ?? [],
    request.allowedPaths,
  );

  const redacted = parseResult(redactSecrets(result));
  assertOutputSize(redacted, maxOutputBytes);
  return redacted;
}
