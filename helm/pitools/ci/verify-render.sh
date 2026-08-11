#!/usr/bin/env bash
set -euo pipefail

chart_dir="${1:-helm/pitools}"
values_file="${chart_dir}/ci/values.yaml"
rendered="$(mktemp)"
core_worker_rendered="$(mktemp)"
worker_rendered="$(mktemp)"
network_policy_rendered="$(mktemp)"
provider_worker_rendered="$(mktemp)"
trap 'rm -f "${rendered}" "${core_worker_rendered}" "${worker_rendered}" "${network_policy_rendered}" "${provider_worker_rendered}"' EXIT

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
helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" \
  --show-only templates/networkpolicy.yaml >"${network_policy_rendered}"
helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" \
  --set piWorker.provider.enabled=true \
  --set piWorker.provider.existingSecret=pitools-provider \
  --set piWorker.provider.secretEnv[0].name=OPENAI_API_KEY \
  --set piWorker.provider.secretEnv[0].key=openai-api-key \
  --show-only templates/pi-worker-deployment.yaml >"${provider_worker_rendered}"

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
if grep -Eq 'PITOOLS_PI_ENABLE_SDK|pitools-provider|OPENAI_API_KEY' "${worker_rendered}"; then
  echo 'render check failed: provider SDK or secret leaked into the default runner' >&2
  exit 1
fi
for pattern in \
  'name: PITOOLS_PI_ENABLE_SDK' \
  'value: "1"' \
  'name: "OPENAI_API_KEY"' \
  'name: "pitools-provider"' \
  'key: "openai-api-key"'; do
  if ! grep -Fq -- "${pattern}" "${provider_worker_rendered}"; then
    echo "render check failed: provider-enabled Pi worker is missing ${pattern}" >&2
    exit 1
  fi
done
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
if helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" \
  --set piWorker.provider.enabled=true \
  --set piWorker.provider.existingSecret=pitools-application \
  --set 'piWorker.provider.secretEnv[0].name=OPENAI_API_KEY' \
  --set 'piWorker.provider.secretEnv[0].key=openai-api-key' \
  --show-only templates/pi-worker-deployment.yaml >/dev/null 2>&1; then
  echo 'render check failed: provider SDK can reuse the application Secret' >&2
  exit 1
fi
if helm template pitools "${chart_dir}" \
  --namespace pitools-system \
  --values "${values_file}" \
  --set piWorker.provider.enabled=true \
  --set piWorker.provider.existingSecret=pitools-provider \
  --set 'piWorker.provider.secretEnv[0].name=GITHUB_TOKEN' \
  --set 'piWorker.provider.secretEnv[0].key=provider-key' \
  --show-only templates/pi-worker-deployment.yaml >/dev/null 2>&1; then
  echo 'render check failed: provider SDK can receive a GitHub credential environment name' >&2
  exit 1
fi

pi_network_policy="$(sed -n '/name: pitools-pi-worker/,$p' "${network_policy_rendered}")"
if [[ -z "${pi_network_policy}" ]]; then
  echo 'render check failed: isolated Pi worker NetworkPolicy is absent' >&2
  exit 1
fi
for pattern in \
  'component: pi-worker' \
  'policyTypes:' \
  '- Egress' \
  'cidr: 192.0.2.12/32' \
  'port: 443'; do
  if ! grep -Fq -- "${pattern}" <<<"${pi_network_policy}"; then
    echo "render check failed: Pi worker provider egress is not explicitly allowlisted (${pattern})" >&2
    exit 1
  fi
done
if grep -Fq -- '0.0.0.0/0' <<<"${pi_network_policy}"; then
  echo 'render check failed: Pi worker provider egress is unrestricted' >&2
  exit 1
fi

echo "Helm security render check passed"
