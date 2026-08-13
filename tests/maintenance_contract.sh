#!/usr/bin/env bash
set -euo pipefail

repository_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/pitools-maintenance.XXXXXX")"
trap 'rm -rf -- "$fixture_root"' EXIT

mkdir -p "$fixture_root/src" "$fixture_root/workers/pi/dist" "$fixture_root/target/debug"
printf '[package]\nname = "maintenance-fixture"\nversion = "0.0.0"\nedition = "2021"\n' > "$fixture_root/Cargo.toml"
printf 'fn main() {}\n' > "$fixture_root/src/main.rs"
printf '{"name":"maintenance-fixture","scripts":{"clean":"rm -rf dist"}}\n' > "$fixture_root/workers/pi/package.json"
printf 'generated\n' > "$fixture_root/target/debug/generated"
printf 'generated\n' > "$fixture_root/workers/pi/dist/generated"

PITOOLS_MAINTENANCE_ROOT="$fixture_root" bash "$repository_root/scripts/maintenance.sh"

if [[ -e "$fixture_root/target" ]]; then
  echo "maintenance left Cargo target artifacts behind" >&2
  exit 1
fi
if [[ -e "$fixture_root/workers/pi/dist" ]]; then
  echo "maintenance left Pi worker dist artifacts behind" >&2
  exit 1
fi

echo "maintenance contract passed"
