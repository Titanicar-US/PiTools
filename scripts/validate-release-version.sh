#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
release_tag="${1:-}"

if [[ ! "${release_tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "release tag must match vMAJOR.MINOR.PATCH" >&2
  exit 2
fi

release_version="${release_tag#v}"
rust_version="$(sed -nE 's/^version = "([^"]+)"/\1/p' "${repo_root}/Cargo.toml" | head -n 1)"
worker_version="$(sed -nE 's/^[[:space:]]*"version": "([^"]+)",?$/\1/p' "${repo_root}/workers/pi/package.json" | head -n 1)"

if [[ -z "${rust_version}" || -z "${worker_version}" ]]; then
  echo "release package versions could not be read" >&2
  exit 2
fi

if [[ "${rust_version}" != "${release_version}" ]]; then
  echo "release tag ${release_tag} does not match Cargo package version ${rust_version}" >&2
  exit 1
fi
if [[ "${worker_version}" != "${release_version}" ]]; then
  echo "release tag ${release_tag} does not match Pi worker version ${worker_version}" >&2
  exit 1
fi
