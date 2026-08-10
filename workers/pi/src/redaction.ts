const REDACTED = "[REDACTED]";

const SECRET_PATTERNS = [
  /\bgh[pousr]_[A-Za-z0-9_-]+\b/g,
  /\bgithub_pat_[A-Za-z0-9_]+\b/g,
  /\bsk-[A-Za-z0-9_-]{8,}\b/g,
  /\bBearer\s+[A-Za-z0-9._~+/=-]+\b/gi,
  /\bAKIA[0-9A-Z]{16}\b/g,
  /\bnpm_[A-Za-z0-9_-]{10,}\b/g,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/g,
  /-----BEGIN [A-Z0-9 ]+-----[\s\S]*?-----END [A-Z0-9 ]+-----/g,
];

const SENSITIVE_KEY = /(?:authorization|api[_-]?key|password|private[_-]?key|secret|token)/i;
const SENSITIVE_ASSIGNMENT =
  /(\b(?:authorization|api[_-]?key|password|private[_-]?key|secret|token)\s*[:=]\s*)[^\s,;]+/gi;

function redactString(value: string): string {
  const redacted = SECRET_PATTERNS.reduce(
    (redacted, pattern) => redacted.replace(pattern, REDACTED),
    value,
  );
  return redacted.replace(SENSITIVE_ASSIGNMENT, `$1${REDACTED}`);
}

function redactValue(value: unknown, seen: WeakMap<object, unknown>): unknown {
  if (typeof value === "string") {
    return redactString(value);
  }
  if (value instanceof Error) {
    return redactString(String(value));
  }
  if (value === null || typeof value !== "object") {
    return value;
  }

  const previous = seen.get(value);
  if (previous !== undefined) {
    return previous;
  }

  if (Array.isArray(value)) {
    const redacted: unknown[] = [];
    seen.set(value, redacted);
    for (const item of value) {
      redacted.push(redactValue(item, seen));
    }
    return redacted;
  }

  const redacted: Record<string, unknown> = {};
  seen.set(value, redacted);
  for (const [key, item] of Object.entries(value)) {
    redacted[key] = SENSITIVE_KEY.test(key) ? REDACTED : redactValue(item, seen);
  }
  return redacted;
}

export function redactSecrets(value: unknown): unknown {
  return redactValue(value, new WeakMap<object, unknown>());
}
