#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/pitools-flux-contract.XXXXXX")"
trap 'rm -rf -- "${test_root}"' EXIT

source_bundle="${test_root}/source/platform/targets/k8s-flux/apps/pitools"
target_root="${test_root}/target"
target_bundle="${target_root}/platform/targets/k8s-flux/apps/pitools"
target_aggregate="${target_root}/platform/targets/k8s-flux/apps/kustomization.yaml"
mkdir -p "${source_bundle}" "${target_root}/platform/targets/k8s-flux/apps"

cat >"${source_bundle}/kustomization.yaml" <<'EOF'
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
- app.yaml
- route.yaml
- networkpolicy.yaml
EOF

cat >"${source_bundle}/app.yaml" <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: pitools
  namespace: pitools-system
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: pitools
  template:
    metadata:
      labels:
        app.kubernetes.io/name: pitools
    spec:
      containers:
        - name: pitools
          image: ghcr.io/titanicar-us/pitools@sha256:50c8ae460d0f873f35138dbc7a04d00f9bf8465f434aebd7169410997a397c3d
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: pitools-postgres
                  key: database-url
        - name: pi-worker
          image: ghcr.io/titanicar-us/pitools-runner@sha256:35d01c8332d0ea62b0df5dd67d6c782a09b7c4b1dbe3c775b78fa8391e391fe4
          env:
            - name: NATS_URL
              valueFrom:
                secretKeyRef:
                  name: pitools-nats
                  key: nats-url
---
apiVersion: v1
kind: Service
metadata:
  name: pitools
  namespace: pitools-system
spec:
  selector:
    app.kubernetes.io/name: pitools
  ports:
    - name: http
      port: 8080
      targetPort: 8080
EOF

cat >"${source_bundle}/route.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: pitools
  namespace: pitools-system
spec:
  parentRefs:
    - name: approved-gateway
  hostnames:
    - hooks.example.test
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /github/webhook
      backendRefs:
        - name: pitools
          port: 8080
EOF

cat >"${source_bundle}/networkpolicy.yaml" <<'EOF'
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: pitools
  namespace: pitools-system
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: pitools
  policyTypes:
    - Ingress
    - Egress
EOF

cat >"${target_aggregate}" <<'EOF'
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
- existing-app
EOF

printf 'keep this unrelated target file\n' >"${target_root}/platform/targets/k8s-flux/apps/README.md"

expect_failure() {
  if python3 "${repo_root}/scripts/publish-flux.py" validate --source "${1}" >/dev/null 2>&1; then
    echo "expected validation failure for ${1}" >&2
    exit 1
  fi
}

cp -R "${source_bundle}" "${test_root}/invalid"
sed -i.bak 's#ghcr.io/titanicar-us/pitools@sha256:[0-9a-f]*#ghcr.io/titanicar-us/pitools:latest#' "${test_root}/invalid/app.yaml"
rm -f "${test_root}/invalid/app.yaml.bak"
expect_failure "${test_root}/invalid"

cp -R "${source_bundle}" "${test_root}/secret-invalid"
printf '%s\n' 'kind: Secret' >>"${test_root}/secret-invalid/route.yaml"
expect_failure "${test_root}/secret-invalid"

cp -R "${source_bundle}" "${test_root}/env-invalid"
printf '%s\n' 'not-a-secret' >"${test_root}/env-invalid/.env"
expect_failure "${test_root}/env-invalid"

cp -R "${source_bundle}" "${test_root}/missing-invalid"
rm "${test_root}/missing-invalid/route.yaml"
expect_failure "${test_root}/missing-invalid"

cp -R "${source_bundle}" "${test_root}/symlink-invalid"
rm "${test_root}/symlink-invalid/route.yaml"
ln -s app.yaml "${test_root}/symlink-invalid/route.yaml"
expect_failure "${test_root}/symlink-invalid"

python3 "${repo_root}/scripts/publish-flux.py" sync \
  --source "${source_bundle}" \
  --destination "${target_bundle}" \
  --aggregate "${target_aggregate}" >/dev/null

for file in kustomization.yaml app.yaml route.yaml networkpolicy.yaml; do
  test -f "${target_bundle}/${file}"
done
test "$(cat "${target_root}/platform/targets/k8s-flux/apps/README.md")" = 'keep this unrelated target file'
test "$(grep -Ec '^- pitools$' "${target_aggregate}")" -eq 1

aggregate_before="${test_root}/aggregate-before"
cp "${target_aggregate}" "${aggregate_before}"
python3 "${repo_root}/scripts/publish-flux.py" sync \
  --source "${source_bundle}" \
  --destination "${target_bundle}" \
  --aggregate "${target_aggregate}" >/dev/null
cmp -s "${aggregate_before}" "${target_aggregate}"

if python3 "${repo_root}/scripts/publish-flux.py" sync \
  --source "${source_bundle}" \
  --destination "${test_root}/unsafe/other/pitools" \
  --aggregate "${target_aggregate}" >/dev/null 2>&1; then
  echo 'expected unsafe destination rejection' >&2
  exit 1
fi

echo 'Flux promotion contract passed'
