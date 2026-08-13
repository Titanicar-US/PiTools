#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="${repo_root}/.github/workflows/publish-flux.yml"
makefile="${repo_root}/Makefile"

test -f "${workflow}"
grep -Fq -- 'name: publish-flux' "${workflow}"
grep -Fq -- 'branches: [main]' "${workflow}"
grep -Fq -- 'workflow_dispatch:' "${workflow}"
grep -Fq -- 'github.ref_protected == true' "${workflow}"
grep -Fq -- 'environment: code-pipeline-promotion' "${workflow}"
grep -Fq -- 'CODE_PIPELINE_DEPLOY_TOKEN' "${workflow}"
grep -Fq -- 'Titanicar-US/code_pipeline' "${workflow}"
grep -Fq -- 'scripts/publish-flux.py' "${workflow}"
grep -Fq -- 'platform/targets/k8s-flux/apps/pitools' "${workflow}"
grep -Fq -- 'platform/targets/k8s-flux/apps/kustomization.yaml' "${workflow}"
grep -Fq -- 'automation/pitools-flux-' "${workflow}"
grep -Fq -- 'chore(deploy): sync PiTools Flux bundle' "${workflow}"
grep -Fq -- '--force-with-lease' "${workflow}"
grep -Fq -- 'source_sha' "${workflow}"
grep -Fq -- 'PITOOLS_FLUX_PROMOTION' "${workflow}"

if grep -Eq 'pull_request:|GITHUB_PRIVATE_KEY|GITHUB_WEBHOOK_SECRET|ADMIN_BEARER_TOKEN|KUBECONFIG' "${workflow}"; then
  echo "source promotion must not run untrusted PR code or receive application credentials" >&2
  exit 1
fi

while IFS= read -r action; do
  if [[ ! "${action}" =~ uses:.*@[0-9a-f]{40} ]]; then
    echo "workflow action is not pinned to a full commit SHA: ${action}" >&2
    exit 1
  fi
done < <(grep -E '^[[:space:]]+- uses:' "${workflow}")

grep -Fq $'\tbash tests/flux_promotion_contract.sh' "${makefile}" || {
  echo "make check must run the Flux promotion contract" >&2
  exit 1
}

echo 'Flux workflow contract passed'
