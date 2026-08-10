#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
publish_script="${repo_root}/scripts/publish-release.sh"
temporary_root="$(mktemp -d)"
trap 'rm -rf "${temporary_root}"' EXIT

fake_bin="${temporary_root}/bin"
mkdir -p "${fake_bin}"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\\n" "$@" > "${PITOOLS_TEST_ARGS_FILE}"' \
  >"${fake_bin}/gh"
chmod 755 "${fake_bin}/gh"

if PITOOLS_RELEASE_TAG=v0.1.0 PITOOLS_PUBLISH_CONFIRM=no "${publish_script}" >/dev/null 2>&1; then
  echo "publish must require explicit confirmation" >&2
  exit 1
fi

if PITOOLS_RELEASE_TAG=release-candidate PITOOLS_PUBLISH_CONFIRM=yes "${publish_script}" >/dev/null 2>&1; then
  echo "publish must require a stable semantic-version tag" >&2
  exit 1
fi

if PITOOLS_RELEASE_TAG=v0.1.0 PITOOLS_PUBLISH_CONFIRM=yes \
  PITOOLS_REPOSITORY=someone/else "${publish_script}" >/dev/null 2>&1; then
  echo "publish must refuse a non-canonical repository" >&2
  exit 1
fi

arguments_file="${temporary_root}/arguments"
PITOOLS_RELEASE_TAG=v0.1.0 \
PITOOLS_PUBLISH_CONFIRM=yes \
PITOOLS_REPOSITORY=Titanicar-US/PiTools \
PITOOLS_TEST_ARGS_FILE="${arguments_file}" \
PATH="${fake_bin}:${PATH}" \
  "${publish_script}"

mapfile -t actual_arguments <"${arguments_file}"
expected_arguments=(
  release
  create
  v0.1.0
  --repo
  Titanicar-US/PiTools
  --target
  main
  --generate-notes
)
if [[ "${#actual_arguments[@]}" -ne "${#expected_arguments[@]}" ]]; then
  echo "publish arguments changed unexpectedly" >&2
  exit 1
fi
for index in "${!expected_arguments[@]}"; do
  if [[ "${actual_arguments[${index}]}" != "${expected_arguments[${index}]}" ]]; then
    echo "publish argument ${index} changed unexpectedly" >&2
    exit 1
  fi
done

echo "publish contract passed"
