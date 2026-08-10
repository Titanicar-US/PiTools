#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose_file="${repo_root}/docker-compose.local.yml"
acceptance_root="$(mktemp -d "${TMPDIR:-/tmp}/pitools-accept.XXXXXX")"
acceptance_project="pitools-accept-$(openssl rand -hex 4)"
acceptance_key="${acceptance_root}/github-app-private-key.pem"
http_port="${PITOOLS_ACCEPTANCE_HTTP_PORT:-18080}"
postgres_port="${PITOOLS_ACCEPTANCE_POSTGRES_PORT:-15432}"
nats_port="${PITOOLS_ACCEPTANCE_NATS_PORT:-14222}"

cleanup() {
  status=$?
  docker compose -p "${acceptance_project}" -f "${compose_file}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  case "${acceptance_root}" in
    "${TMPDIR:-/tmp}"/pitools-accept.*) rm -rf -- "${acceptance_root}" ;;
    *) echo "refusing to remove unexpected acceptance path: ${acceptance_root}" >&2 ;;
  esac
  exit "${status}"
}
trap cleanup EXIT

for command in cargo curl docker openssl rg; do
  command -v "${command}" >/dev/null 2>&1 || {
    echo "required command is missing: ${command}" >&2
    exit 1
  }
done

acceptance_token="$(openssl rand -hex 16)"
acceptance_secret="$(openssl rand -hex 24)"
acceptance_password="$(openssl rand -hex 16)"
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "${acceptance_key}" >/dev/null 2>&1
acceptance_hash="$(printf '%s' "${acceptance_token}" | cargo run --quiet --manifest-path "${repo_root}/Cargo.toml" -- hash-token)"

export PITOOLS_POSTGRES_PASSWORD="${acceptance_password}"
export PITOOLS_GITHUB_APP_ID=123456
export PITOOLS_GITHUB_PRIVATE_KEY_FILE="${acceptance_key}"
export PITOOLS_GITHUB_WEBHOOK_SECRET="${acceptance_secret}"
export PITOOLS_ADMIN_BEARER_TOKEN_HASH="${acceptance_hash}"
export PITOOLS_HTTP_PORT="${http_port}"
export PITOOLS_POSTGRES_PORT="${postgres_port}"
export PITOOLS_NATS_PORT="${nats_port}"

docker compose -p "${acceptance_project}" -f "${compose_file}" config --quiet
docker compose -p "${acceptance_project}" -f "${compose_file}" up -d --build postgres nats pitools pitools-worker

ready_attempt=0
for attempt in $(seq 1 60); do
  ready_attempt="${attempt}"
  if curl --fail --silent "http://127.0.0.1:${http_port}/healthz" >/dev/null 2>&1 \
    && curl --fail --silent "http://127.0.0.1:${http_port}/readyz" >/dev/null 2>&1; then
    break
  fi
  if [[ "${attempt}" == 60 ]]; then
    docker compose -p "${acceptance_project}" -f "${compose_file}" ps
    docker compose -p "${acceptance_project}" -f "${compose_file}" logs --no-color pitools | tail -n 60
    exit 1
  fi
  sleep 2
done

health="$(curl --fail --silent "http://127.0.0.1:${http_port}/healthz")"
ready="$(curl --fail --silent "http://127.0.0.1:${http_port}/readyz")"
metrics_before="$(curl --fail --silent "http://127.0.0.1:${http_port}/metrics")"
unauthorized_status="$(curl --silent --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${http_port}/api/v1/events")"
payload='{"action":"opened","installation":{"id":9,"account":{"login":"Titanicar-US","type":"Organization"}},"repository":{"id":42},"pull_request":{"id":1001,"number":7,"title":"Ready PR","html_url":"https://github.com/Titanicar-US/PiTools/pull/7","state":"open","merged":false,"draft":false,"updated_at":"2026-08-10T00:00:00Z","user":{"login":"author"},"head":{"sha":"head","ref":"feature"},"base":{"sha":"base","ref":"main"}}}'
signature="$(printf '%s' "${payload}" | openssl dgst -sha256 -hmac "${acceptance_secret}" -hex | sed 's/.* //')"
webhook_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "x-hub-signature-256: sha256=${signature}" \
  --header 'x-github-delivery: acceptance-delivery-1' \
  --header 'x-github-event: pull_request' \
  --header 'content-type: application/json' \
  --data "${payload}" "http://127.0.0.1:${http_port}/github/webhook")"
duplicate_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "x-hub-signature-256: sha256=${signature}" \
  --header 'x-github-delivery: acceptance-delivery-1' \
  --header 'x-github-event: pull_request' \
  --header 'content-type: application/json' \
  --data "${payload}" "http://127.0.0.1:${http_port}/github/webhook")"
watchlist="$(curl --fail --silent --header "Authorization: Bearer ${acceptance_token}" \
  "http://127.0.0.1:${http_port}/api/v1/watchlist?limit=100")"
events_after="$(curl --fail --silent --header "Authorization: Bearer ${acceptance_token}" \
  "http://127.0.0.1:${http_port}/api/v1/events?limit=100")"
metrics_after="$(curl --fail --silent "http://127.0.0.1:${http_port}/metrics")"

printf 'statuses health=%s ready=%s unauthorized=%s webhook=%s duplicate=%s ready_attempt=%s\n' \
  "${health}" "${ready}" "${unauthorized_status}" "${webhook_status}" "${duplicate_status}" "${ready_attempt}"
[[ "${health}" == ok ]]
[[ "${ready}" == ready ]]
[[ "${unauthorized_status}" == 401 ]]
[[ "${webhook_status}" == 202 ]]
[[ "${duplicate_status}" == 208 ]]
printf '%s' "${watchlist}" | rg -q '"repository_id":42|"number":7'
printf '%s' "${events_after}" | rg -q 'acceptance-delivery-1'
printf '%s' "${metrics_before}${metrics_after}" | rg -q 'pitools_webhook_received_total'
printf 'compose_acceptance=passed\nwatchlist_and_event_persistence=passed\n'
