import { posix, win32 } from "node:path";

export function assertCanonicalRepositoryPath(candidate: string): void {
  const segments = candidate.split("/");
  const isInvalid =
    candidate.length === 0 ||
    candidate.includes("\0") ||
    candidate.includes("\\") ||
    posix.isAbsolute(candidate) ||
    win32.isAbsolute(candidate) ||
    posix.normalize(candidate) !== candidate ||
    segments.some((segment) => segment.length === 0 || segment === "." || segment === "..");

  if (isInvalid) {
    throw new Error(`PiTools path is not a canonical repository-relative path: ${candidate}`);
  }
}

export function assertAllowedPaths(proposedPaths: readonly string[], allowedPaths: readonly string[]): void {
  for (const allowedPath of allowedPaths) {
    assertCanonicalRepositoryPath(allowedPath);
  }

  const allowlist = new Set(allowedPaths);
  for (const proposedPath of proposedPaths) {
    assertCanonicalRepositoryPath(proposedPath);
    if (!allowlist.has(proposedPath)) {
      throw new Error(`PiTools result contains a path outside the allowlist: ${proposedPath}`);
    }
  }
}
