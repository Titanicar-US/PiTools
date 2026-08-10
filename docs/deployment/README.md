# Deployment packaging

PiTools ships two images and one Helm chart:

- `Dockerfile` builds only the non-root Rust control plane. It contains no Node runtime, Pi source, or worker dependencies, and includes bubblewrap for credential-free repository validation.
- `runner/Dockerfile` builds the non-root, short-lived Pi repair worker. It has Git and SSH client tooling but no Docker daemon, Kubernetes client, GitHub App private key, webhook secret, or admin token.
- `helm/pitools` deploys separate HTTP core, Rust durable-worker, and Pi worker workloads. The Rust worker owns GitHub credentials and durable mutations; repository validation is copied into a `.git`-free bubblewrap snapshot with no network namespace and no application-secret mount. The Pi worker uses a minimal NATS request/reply transport and receives no application credentials.
- PostgreSQL, NATS, application secrets, DNS, TLS, and Flux reconciliation remain externally owned.

The base images are pinned by both readable version tag and multi-platform manifest digest. Refresh a digest only after reviewing the upstream image and rebuilding both architectures used by the target cluster.

Run the runner with a read-only root filesystem, a writable `noexec,nosuid,nodev` tmpfs at `/tmp`, a bounded writable worktree at `/workspace`, all Linux capabilities dropped, and only the approved network path. The caller supplies one protocol request on standard input and consumes the validated result on standard output.

## Local Compose

`docker-compose.local.yml` starts the HTTP core, Rust worker, and optional Pi worker with local PostgreSQL and NATS. It is not a production topology. All published ports bind to loopback.

Generate an App Manifest for the approved HTTPS hostname with `pitools manifest --base-url https://<approved-hostname>` and complete the GitHub App creation/install flow before sending signed events. The generated `/github/manifest/callback` exchanges GitHub's one-time code and returns the App ID, PEM, and webhook secret with `Cache-Control: no-store`; copy them directly into the external secret manager and discard the response. PiTools does not persist App credentials.

Set non-production values in your shell; do not commit them:

```bash
export PITOOLS_POSTGRES_PASSWORD='<local-only-password>'
export PITOOLS_GITHUB_APP_ID='<non-production-app-id>'
export PITOOLS_GITHUB_PRIVATE_KEY_FILE='<absolute-path-to-local-private-key.pem>'
export PITOOLS_GITHUB_WEBHOOK_SECRET='<local-only-webhook-secret>'
export PITOOLS_ADMIN_BEARER_TOKEN_HASH='<argon2id-hash-for-local-use>'
docker compose -f docker-compose.local.yml up --build
```

Run the bounded local webhook and persistence smoke test with `make acceptance`. It generates temporary non-production credentials, builds the core image, starts PostgreSQL and NATS, verifies health/readiness and admin authorization, sends and replays one signed pull-request delivery, reads the watchlist and event ledger, checks webhook metrics, and tears down its uniquely named Compose project. Override `PITOOLS_ACCEPTANCE_HTTP_PORT`, `PITOOLS_ACCEPTANCE_POSTGRES_PORT`, or `PITOOLS_ACCEPTANCE_NATS_PORT` when the default loopback ports are occupied.

Stop the stack with `docker compose -f docker-compose.local.yml down`. Add `--volumes` only when intentionally discarding the local PostgreSQL data.

The Pi runner receives no PiTools application secrets. For a one-shot protocol check, send one JSON request on standard input:

```bash
printf '%s\n' '<validated-pi-job-json>' \
  | docker compose -f docker-compose.local.yml --profile worker run --rm -T \
      -e PITOOLS_PI_TRANSPORT=stdin pi-worker
```

The default diagnosis-only runtime requires no model credential. Enabling the Pi SDK or supplying provider credentials is a separate, caller-owned authorization boundary; do not add those values to this Compose file. A provider-backed worker may return typed unified patches, but Rust applies them only after exact path validation, a fresh PR-head check, the configured validation commands, and an authorized Check Run approval.

## Image publication

The image workflow validates source, the Helm render, and independent core and runner image builds on pull requests before any registry login or publication. Pull-request image validation uses the runner Docker CLI for Linux amd64; an approved release tag uses the protected BuildKit publisher to produce Linux amd64 and arm64 images with provenance and SBOM attestations. Consume the registry-reported image digest, not the mutable tag.

## Helm rendering

The chart deliberately has no deployable image or secret defaults. Supply a values file maintained by the infrastructure repository:

```bash
helm lint helm/pitools -f '<path-to-environment-values.yaml>'
helm template pitools helm/pitools --namespace '<namespace>' -f '<path-to-environment-values.yaml>'
helm/pitools/ci/verify-render.sh
```

NetworkPolicy is fail-closed. Environment values must identify ingress peers and DNS, PostgreSQL, NATS, and any approved HTTPS egress peers. Kubernetes NetworkPolicy does not accept DNS names; use namespace/pod selectors for in-cluster services or stable, platform-approved CIDRs for external services.

Provider access is a separate Pi-worker egress contract: set `piWorker.networkPolicy.egress.https.enabled=true` and provide only the approved model-provider or proxy peers. The core `networkPolicy.egress.https` setting does not grant the Pi worker provider access, and the chart defaults both paths to deny.

Provider-backed SDK execution is also opt-in. Set `piWorker.provider.enabled=true`, reference a separate external Secret with `piWorker.provider.existingSecret`, and map only provider credential keys through `piWorker.provider.secretEnv`. The chart rejects reuse of the application Secret and rejects GitHub, database, NATS, or PiTools control-plane environment names. Leave the provider disabled for deterministic diagnosis-only operation.

With `piWorker.enabled=true`, the chart runs the long-lived NATS worker. Keep NATS egress restricted to the approved NATS peer; the worker still has no GitHub API egress or application secret. The Rust worker owns durable job state, request binding, timeouts, and mutation approval. The HTTP server also performs scoped periodic open-PR inventory so a missed webhook does not permanently remove a PR from observation once its installation/repository record exists.
