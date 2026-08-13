#!/usr/bin/env bash
set -euo pipefail

script_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
maintenance_root="${PITOOLS_MAINTENANCE_ROOT:-$script_root}"
maintenance_root="$(CDPATH= cd -- "$maintenance_root" && pwd -P)"

if [[ "$maintenance_root" == "/" || ! -f "$maintenance_root/Cargo.toml" ]]; then
  echo "maintenance root must be a non-root directory containing Cargo.toml" >&2
  exit 1
fi

cargo_bin="${CARGO:-cargo}"
npm_bin="${NPM:-npm}"

CARGO_TARGET_DIR="$maintenance_root/target" "$cargo_bin" clean --manifest-path "$maintenance_root/Cargo.toml"

if [[ -f "$maintenance_root/workers/pi/package.json" ]]; then
  "$npm_bin" --prefix "$maintenance_root/workers/pi" run clean
fi

echo "PiTools generated artifacts cleaned from $maintenance_root"
