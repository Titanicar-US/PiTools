# PiTools

PiTools is a self-hosted GitHub App that keeps pull requests moving toward human merge readiness. It watches pull requests, reviews, comments, checks, and GitHub Actions; records an auditable state; and uses deterministic repair before bounded Pi workflows where reasoning is required.

The project is in active bootstrap. The control-plane boundary is intentionally fail-closed: PiTools never merges a pull request, and the Pi worker never receives GitHub App credentials or writes to GitHub directly.

The Pi worker defaults to a deterministic diagnosis-only runtime and communicates with the Rust worker over a bounded NATS request/reply subject. Set `PITOOLS_PI_ENABLE_SDK=1` only in the isolated worker workload after configuring its provider credentials; the SDK runtime is toolless and its output still requires Rust-side validation and approval.

## Capabilities

- GitHub App Manifest setup and installation validation.
- Signed webhook intake with delivery deduplication, installation suspend/delete and repository-removal handling, open-PR inventory recovery, and periodic reconciliation.
- Durable PR watchlist, protected-branch-aware readiness reasons, per-PR job/audit/delivery history in the API and CLI, and operator audit history.
- Living progress comments with a visible approval fingerprint, final change summaries, and GitHub-native Approve/Skip/Cancel Check Run actions.
- Deterministic configured-automation feedback repair after an authorized plan approval, with bounded applied/rejected replies on original review threads or PR-level outcome comments for issue comments.
- GitHub Actions diagnosis with bounded check-run output, first-page annotations, and plain-text job logs redacted before Pi receives them; typed, allowlisted Pi patch proposals are applied only after approval in an ephemeral worktree, credential-free bubblewrap validation, exact staged-path verification, and a fresh-head check.
- Pi diagnosis requests include only bounded contents of configured repair paths at the exact PR head; the Pi sidecar receives no repository volume or GitHub credential.
- Explicit PR stack partitioning and ordering, approved base-branch updates applied only after rebase safety checks, and branch-history rebases; force-with-lease rewrites remain disabled unless both `allow_bot_force_push: true` and an exact `bot_owned_branches` entry authorize the branch.
- Readiness preserves GitHub-required status-check app identity, unions repository policy checks with protected-branch checks, and counts current approvals while identifying stale or missing latest-push approvals.

## Local prerequisites

- Rust 1.97.1 with Cargo, rustfmt, and Clippy.
- Node.js 26 or newer and npm.
- Helm 3 for the local chart render contract.
- PostgreSQL 17 or newer for integration tests and local operation.
- NATS 2.10 or newer for wakeup notifications.

Run `make install` to fetch dependencies. Run `make check` for the changed-scope local gate.

## Configuration

Copy `.env.example` to `.env` and provide values through a local secret manager or environment. Never commit a private key, webhook secret, bearer token, model credential, or repository secret.

The required values are `DATABASE_URL`, `GITHUB_APP_ID`, `GITHUB_PRIVATE_KEY_PATH`, `GITHUB_WEBHOOK_SECRET`, and `ADMIN_BEARER_TOKEN_HASH`. `NATS_URL` defaults to `nats://127.0.0.1:4222`; `PITOOLS_BIND_ADDRESS` defaults to `0.0.0.0:8080`; `PITOOLS_RECONCILE_INTERVAL_SECONDS` defaults to 300 and must be between 30 and 86400.

Generate the admin hash without putting the token in shell history: `printf '%s' 'local-token' | pitools hash-token`.

Generate the GitHub App Manifest JSON with `pitools manifest --base-url https://<approved-hostname>`, use it in GitHub's App creation flow, and let the generated `/github/manifest/callback` exchange the one-time code. Copy the returned App ID, PEM, and webhook secret into the deployment's external secret manager, then discard the browser response and run `pitools doctor` after installing the App on selected repositories. The callback does not persist credentials. The full future-facing permission matrix is intentional; runtime policy and installation-token scoping still fail closed.

`pitools doctor` authenticates as the configured App, verifies GitHub's `/app` identity matches `GITHUB_APP_ID`, and reports the visible installation count and account names without printing credentials. An identity mismatch or GitHub API failure is fatal; NATS may be reported as degraded because the worker can fall back to durable Postgres polling.

The generated manifest requests `administration:read`, `metadata:read`, `pull_requests:write`, `issues:write`, `checks:write`, `statuses:read`, `actions:read`, and `contents:write`, plus the PR, review, check, status, and workflow webhook events. The administration read permission lets readiness checks inspect protected-branch requirements. Installation tokens are short-lived and should be scoped to selected repositories; PiTools never uses a broad personal access token. Suspension, deletion, and repository-removal events stop reconciliation until access is restored and inventory is refreshed. Periodic inventory can recover open PRs after a missed webhook and fresh reads retire watched rows after close/merge.

The operator CLI reads the same durable state used by the server: `pitools watchlist [--json]`, `pitools events [--limit 50] [--json]`, and `pitools audit [--limit 50] [--json]`. Use `pitools reconcile --repository-id <id> --pull-request-number <number>` to enqueue a refresh and `pitools cancel <job-id>` to request cancellation. Event output contains delivery metadata and hashes, not raw webhook payloads; list limits are capped at 100.

## Repository policy

Repositories may commit a `.pitools.yml` policy. The policy controls configured automation actors, readiness requirements, validation commands, reconciliation interval, explicit stack parents, and opt-in branch rewrite rules. Start from [the example policy](docs/policy.example.yml); service safety invariants always win.

`require_approval` defaults to `true`. Typed CI patches always require an explicit Check Run approval even if repository policy disables the general approval setting; a provider result that does not declare approval is rejected. CI repairs additionally require exact files in `repair_allowed_paths`; Pi-proposed shell commands are informational only and are never executed. `stack_parents` can declare a pull request's parent by number so approved stack jobs can correct PR base branches and rebase branches deterministically. History rewrites require `allow_bot_force_push` plus an exact `bot_owned_branches` allowlist entry and always use `--force-with-lease` against the fresh observed head.

The Skip and Cancel actions remain available while a plan is waiting for approval. Skipping advances only the selected planned item and never authorizes the remaining items; approval is still required before any later mutation runs.

## Deployment

The `Dockerfile`, `runner/`, `workers/pi/`, and `helm/pitools/` directories provide the application packaging. Local Compose is for development and acceptance only. The protected `build-image` workflow publishes multi-architecture core and runner images on approved `v*` tags and records immutable GHCR digests in its job summaries. Production dev01 integration is owned by the Flux infrastructure repository and should consume those digests through the Helm chart with external PostgreSQL and NATS endpoints.

After a human merges the validated PR into `main`, publish an explicit release from the canonical repository with `make publish PITOOLS_RELEASE_TAG=v0.1.0 PITOOLS_PUBLISH_CONFIRM=yes`. The command only accepts stable semantic-version tags, requires the confirmation flag, creates the GitHub release against `main`, and lets the protected image workflow produce the immutable digests for the Flux handoff. It is intentionally not run during local validation.

Provider-backed Pi execution is disabled by default. The Helm chart enables it only through a separate provider Secret, explicit `secretEnv` mappings, and a Pi-worker-only HTTPS egress allowlist; the application Secret and GitHub/control-plane environment names are rejected at render time.

The server exposes low-cardinality Prometheus counters at `/metrics`; keep that route internal to the dev01 monitoring network when the ingress is configured.

## License

PiTools is licensed under Apache-2.0. See [LICENSE](LICENSE).
