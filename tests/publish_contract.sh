#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
publish_script="${repo_root}/scripts/publish-release.sh"
image_workflow="${repo_root}/.github/workflows/build-image.yml"
makefile="${repo_root}/Makefile"
dockerfile="${repo_root}/Dockerfile"
temporary_root="$(mktemp -d)"
trap 'rm -rf "${temporary_root}"' EXIT

if ! grep -Fq $'\tcargo fmt --all -- --check' "${makefile}"; then
  echo "make check must enforce Rust formatting" >&2
  exit 1
fi
if ! grep -Fq $'\thelm/pitools/ci/verify-render.sh' "${makefile}"; then
  echo "make check must validate the Helm render contract" >&2
  exit 1
fi

if ! grep -Fq -- '  pull_request:' "${image_workflow}"; then
  echo "image validation must run on pull requests" >&2
  exit 1
fi
validate_image_block="$(sed -n '/^  validate-image:/,/^  publish-image:/p' "${image_workflow}")"
if ! grep -Fq -- '          docker build' <<<"${validate_image_block}" ||
  ! grep -Fq -- '            --platform linux/amd64' <<<"${validate_image_block}"; then
  echo "pull-request image validation must build with the runner Docker CLI" >&2
  exit 1
fi
if grep -Eq 'docker/(setup-buildx|build-push)-action@' <<<"${validate_image_block}"; then
  echo "pull-request image validation must not invoke the release Docker actions" >&2
  exit 1
fi
if ! grep -Fq -- "    if: github.ref_type == 'tag' && startsWith(github.ref_name, 'v')" "${image_workflow}"; then
  echo "image publication must run only for v-prefixed tags" >&2
  exit 1
fi
if ! grep -Fq -- '          RELEASE_TAG: ${{ github.ref_name }}' "${image_workflow}"; then
  echo "image publication must validate the release tag" >&2
  exit 1
fi
if ! grep -Fq -- '          [[ "${RELEASE_TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]' "${image_workflow}"; then
  echo "image publication must require an exact semantic release tag" >&2
  exit 1
fi

if ! grep -Fq -- 'FROM --platform=$BUILDPLATFORM' "${dockerfile}" ||
  ! grep -Fq -- 'aarch64-unknown-linux-gnu' "${dockerfile}" ||
  ! grep -Fq -- 'libc6-dev-arm64-cross' "${dockerfile}" ||
  ! grep -Fq -- 'linux-libc-dev-arm64-cross' "${dockerfile}" ||
  ! grep -Fq -- 'CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER' "${dockerfile}" ||
  ! grep -Fq -- 'COPY --from=rust-builder /out/pitools' "${dockerfile}"; then
  echo "multi-architecture core images must cross-compile Rust outside target emulation" >&2
  exit 1
fi
if ! grep -Eq -- 'linux-libc-dev-arm64-cross;[[:space:]]*\\$' "${dockerfile}"; then
  echo "arm64 package installation must continue the Dockerfile shell command" >&2
  exit 1
fi
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
