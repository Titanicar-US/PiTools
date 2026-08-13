#!/usr/bin/env bash
set -euo pipefail

repository="${PITOOLS_REPOSITORY:-Titanicar-US/PiTools}"
release_tag="${PITOOLS_RELEASE_TAG:-}"
confirmation="${PITOOLS_PUBLISH_CONFIRM:-}"

if [[ "${repository}" != "Titanicar-US/PiTools" ]]; then
  echo "refusing to publish outside Titanicar-US/PiTools" >&2
  exit 2
fi

if [[ "${confirmation}" != "yes" ]]; then
  echo "set PITOOLS_PUBLISH_CONFIRM=yes to publish an explicit release" >&2
  exit 2
fi

if [[ ! "${release_tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "PITOOLS_RELEASE_TAG must match vMAJOR.MINOR.PATCH" >&2
  exit 2
fi

bash "$(dirname "${BASH_SOURCE[0]}")/validate-release-version.sh" "${release_tag}"

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI is required to publish a release" >&2
  exit 2
fi

exec gh release create "${release_tag}" \
  --repo "${repository}" \
  --target main \
  --generate-notes
