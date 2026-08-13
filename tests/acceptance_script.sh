#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/pitools-acceptance-test.XXXXXX")"
fake_bin="${test_root}/bin"
fake_docker_pids="${test_root}/docker-pids"
output_file="${test_root}/output"
mkdir -p "${fake_bin}"
touch "${fake_docker_pids}"

cleanup() {
  while read -r pid; do
    [[ -n "${pid}" ]] && kill -TERM "${pid}" 2>/dev/null || true
  done < "${fake_docker_pids}"
  rm -rf -- "${test_root}"
}
trap cleanup EXIT

cat >"${fake_bin}/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$$" >> "${FAKE_DOCKER_PIDS}"
exec sleep 30
EOF
chmod +x "${fake_bin}/docker"

cat >"${fake_bin}/cargo" <<'EOF'
#!/usr/bin/env bash
printf 'test-token-hash\n'
EOF
chmod +x "${fake_bin}/cargo"

cat >"${fake_bin}/openssl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  rand)
    printf 'test-random\n'
    ;;
  genpkey)
    for argument in "$@"; do
      if [[ "${previous_argument:-}" == '-out' ]]; then
        : >"${argument}"
      fi
      previous_argument="${argument}"
    done
    ;;
  *)
    printf 'test-digest\n'
    ;;
esac
EOF
chmod +x "${fake_bin}/openssl"

cat >"${fake_bin}/curl" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${fake_bin}/curl"

set +e
PATH="${fake_bin}:/usr/bin:/bin" \
FAKE_DOCKER_PIDS="${fake_docker_pids}" \
PITOOLS_DOCKER_TIMEOUT_SECONDS=1 \
bash "${repo_root}/scripts/accept-local-compose.sh" >"${output_file}" 2>&1 &
script_pid=$!
set -e

for _ in $(seq 1 8); do
  process_state="$(ps -p "${script_pid}" -o stat= 2>/dev/null | tr -d ' ' || true)"
  if [[ -z "${process_state}" || "${process_state}" == Z* ]]; then
    break
  fi
  sleep 1
done

process_state="$(ps -p "${script_pid}" -o stat= 2>/dev/null | tr -d ' ' || true)"
if [[ -n "${process_state}" && "${process_state}" != Z* ]]; then
  kill -KILL "${script_pid}" 2>/dev/null || true
  while read -r pid; do
    [[ -n "${pid}" ]] && kill -KILL "${pid}" 2>/dev/null || true
  done < "${fake_docker_pids}"
  wait "${script_pid}" 2>/dev/null || true
  echo "acceptance script exceeded bounded Docker failure timeout" >&2
  exit 1
fi

if ! grep -Eq 'Docker daemon|timed out' "${output_file}"; then
  echo "acceptance script did not report the Docker timeout" >&2
  cat "${output_file}" >&2
  exit 1
fi
