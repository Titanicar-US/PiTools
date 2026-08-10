#!/usr/bin/env bash
set -euo pipefail

chart_dir="${1:-helm/pitools}"
values_file="${chart_dir}/ci/values.yaml"
rendered="$(mktemp)"
core_worker_rendered="$(mktemp)"
worker_rendered="$(mktemp)"
trap 'rm -f "${rendered}" "${core_worker_rendered}" "${worker_rendered}"' EXIT

helm lint "${chart_dir}" --values "${values_file}"
helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" >"${rendered}"
helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" \
  --set piWorker.enabled=true \
  --set piWorker.image.repository=ghcr.io/example/pitools-runner \
  --set piWorker.image.digest=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  --show-only templates/pi-worker-deployment.yaml >"${worker_rendered}"
helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" \
  --show-only templates/worker-deployment.yaml >"${core_worker_rendered}"

require_rendered() {
  local pattern="$1"
  local description="$2"
  if ! grep -Eq -- "${pattern}" "${rendered}"; then
    echo "render check failed: ${description}" >&2
    exit 1
  fi
}

reject_rendered() {
  local pattern="$1"
  local description="$2"
  if grep -Eq -- "${pattern}" "${rendered}"; then
    echo "render check failed: ${description}" >&2
    exit 1
  fi
}

require_rendered 'image: "ghcr.io/example/pitools@sha256:a{64}"' 'core image is not digest-pinned'
require_rendered 'automountServiceAccountToken: false' 'service-account token automount is not disabled'
require_rendered 'readOnlyRootFilesystem: true' 'root filesystem is not read-only'
require_rendered 'allowPrivilegeEscalation: false' 'privilege escalation is not disabled'
require_rendered 'runAsNonRoot: true' 'non-root execution is not required'
require_rendered 'runAsUser: 10001' 'expected runtime UID is absent'
require_rendered 'seccompProfile:' 'seccomp profile is absent'
require_rendered 'drop:' 'capability drop list is absent'
require_rendered 'kind: NetworkPolicy' 'fail-closed NetworkPolicy is absent'
reject_rendered '^kind: Secret$' 'chart rendered a Secret instead of an external reference'
if ! grep -Eq 'image: "ghcr.io/example/pitools-runner@sha256:b{64}"' "${worker_rendered}"; then
  echo 'render check failed: optional runner image is not digest-pinned' >&2
  exit 1
fi
if grep -Eq 'pitools-application|GITHUB_|ADMIN_BEARER_TOKEN|github-app-private-key' "${worker_rendered}"; then
  echo 'render check failed: optional runner received an application secret' >&2
  exit 1
fi
if ! grep -Eq 'image: "ghcr.io/example/pitools@sha256:a{64}"' "${core_worker_rendered}"; then
  echo 'render check failed: core worker image is not digest-pinned' >&2
  exit 1
fi
if ! grep -Eq 'GITHUB_PRIVATE_KEY_PATH|github-app-private-key' "${core_worker_rendered}"; then
  echo 'render check failed: core worker is missing its explicit GitHub App key mount' >&2
  exit 1
fi
if grep -Eq 'PITOOLS_PI_TRANSPORT|pi-worker|runner' "${core_worker_rendered}"; then
  echo 'render check failed: core worker contains Pi runner configuration' >&2
  exit 1
fi
for pattern in \
  'automountServiceAccountToken: false' \
  'readOnlyRootFilesystem: true' \
  'allowPrivilegeEscalation: false' \
  'runAsNonRoot: true' \
  'runAsUser: 10001'; do
  if ! grep -Fq -- "${pattern}" "${worker_rendered}"; then
    echo "render check failed: optional runner missing ${pattern}" >&2
    exit 1
  fi
done

echo "Helm security render check passed"
