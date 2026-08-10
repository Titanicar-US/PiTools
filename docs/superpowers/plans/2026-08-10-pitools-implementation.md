# PiTools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the PiTools GitHub App, deterministic repair control plane, isolated Pi worker, safe stack management, and dev01-ready packaging for `Titanicar-US/PiTools`.

**Architecture:** A Rust Axum service owns GitHub authority, Postgres state, webhook/event processing, readiness, policy, comments, Check Run controls, and mutations. A TypeScript Pi sidecar is an untrusted reasoning worker that returns only validated typed results over a restricted NATS request/reply subject. NATS provides best-effort wakeups and sidecar transport; Postgres stores durable jobs, leases, retries, and cancellation.

**Tech Stack:** Rust 1.97 toolchain, Tokio, Axum, SQLx/Postgres, Reqwest, JSON Web Tokens, HMAC/SHA-256, async-nats, Clap, Serde, TypeScript 5.9, Pi SDK 0.82, Valibot, Node test runner, Docker, Helm, Kubernetes, and Cucumber BDD scenarios.

## Global Constraints

- Use Apache-2.0 licensing and ASCII-safe source, comments, Markdown, YAML, and operator output.
- GitHub.com is the only provider target in this release; the App is self-hosted single-tenant.
- The App requests the full future-facing permission set, but no merge, secret export, or unapproved mutation is allowed.
- Postgres is durable source of truth; NATS wakeups and Pi transport are best-effort and non-durable.
- `.pitools.yml` is repository policy; service safety invariants cannot be weakened by repository policy.
- No force-push by default. Repair commits use the App bot identity.
- Pi has provider/allowlisted egress only, no GitHub App/admin secrets, and no direct GitHub mutation capability.
- Every behavior-changing feature has executable BDD scenarios as well as focused unit/contract tests.
- `make check` is the local changed-scope gate. `make check-full` and `make quality-gates` are CI/post-merge commands only.

## Implementation status (2026-08-10)

The initial implementation is present and validated through the local changed-scope gate. Delivered slices include the Rust control plane, GitHub App manifest/webhook handling, durable watchlist and job leases, living work-plan comments with approve/skip/cancel Check Run controls (including approval-wait skips/cancellations), deterministic feedback repair, typed approval-gated CI patch application, safe stack planning/base updates, the Pi sidecar protocol, container/Helm packaging, and the dev01 handoff.

The remaining release prerequisites are external or intentionally fail-closed: GitHub credentials and authenticated UAT, remote/commit/publication authority, dev01/Flux rollout, provider-backed Pi execution, and fork-head write scope. Bounded Actions job-log and check-run annotation evidence is delivered; low-cardinality Prometheus metrics are delivered. The task checkboxes below are the original design checklist; this status and `CONTEXT.md` are the current delivery record.

---

### Task 1: Bootstrap the repository and developer contract

**Files:**
- Create: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`, `workers/pi/`, `Makefile`, `.gitignore`, `LICENSE`, `README.md`, `AGENTS.md`
- Create: `.github/workflows/check.yml`, `.github/workflows/build-image.yml`
- Create: `tests/features/bootstrap.feature`, `tests/bdd.rs`
- Create: `docs/superpowers/specs/2026-08-10-pitools-design.md`

**Interfaces:**
- Produces `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`, `npm --prefix workers/pi test`, and `make check` as stable local commands.
- Produces `pitools server`, `pitools worker`, `pitools doctor`, and `pitools reconcile` command names even when later tasks add their implementations.

- [ ] Write the bootstrap BDD scenario asserting the repository exposes the documented commands and refuses to start without required configuration.
- [ ] Add the Rust workspace and shared dependency versions approved by Dependency Advisor: Tokio 1.53.1, Axum 0.8.9, SQLx 0.9.0, Serde 1.0.229, Serde JSON 1.0.151, Reqwest 0.13.4, JSON Web Token 11.0.0, HMAC 0.13.0, SHA-2 0.11.0, Clap 4.6.4, Tracing 0.1.44, Tracing Subscriber 0.3.23, UUID 1.24.0, Chrono 0.4.45, Thiserror 2.0.19, Anyhow 1.0.104, Dotenvy 0.15.7, Async NATS 0.50.0, Tower HTTP 0.7.0, and Cucumber 0.23.0.
- [ ] Add the TypeScript worker package with TypeScript 5.9.3, Node types 24.13.3, Pi SDK 0.82.1, and Valibot 1.4.2; keep the provider wrapper behind a local interface so tests do not need model credentials.
- [ ] Add Makefile targets: `install`, `check`, `check-full`, `quality-gates`, `clean`, `build`, and `publish`. `make check` runs Rust formatting/lint/tests, Node tests, BDD tests, and manifest validation; `make check-full` runs all tests; `make quality-gates` runs non-test packaging/security checks only.
- [ ] Add README sections for local setup, App Manifest creation, required secrets without secret values, commands, policy file, webhook route, and the dev01 Helm/Flux handoff.
- [ ] Run the focused bootstrap BDD and `make check`.

### Task 2: Add configuration, GitHub App authentication, and safe HTTP foundations

**Files:**
- Create: `src/config.rs`, `src/error.rs`, `src/main.rs`, `src/github/auth.rs`, `src/github/client.rs`, `src/web.rs`
- Test: `tests/config.rs`, `tests/github_auth.rs`, `tests/webhook_http.rs`

**Interfaces:**
- `AppConfig::from_env() -> Result<AppConfig, ConfigError>` validates `DATABASE_URL`, `NATS_URL`, `GITHUB_APP_ID`, `GITHUB_PRIVATE_KEY_PATH`, `GITHUB_WEBHOOK_SECRET`, and `ADMIN_BEARER_TOKEN_HASH` without logging values.
- `GitHubAppAuth::installation_token(installation_id: i64) -> Result<SecretString, GitHubError>` creates a short-lived App JWT and exchanges it for an installation token.
- `POST /github/webhook` accepts the raw request body plus GitHub headers and returns 202 only after signature and delivery-id validation.
- `GET /healthz` checks process liveness; `GET /readyz` checks database and NATS connectivity.

- [ ] Write failing tests for missing configuration, malformed private key, constant-time webhook signature comparison, and invalid delivery headers.
- [ ] Implement environment parsing with secret values held in `secrecy` types and redacted errors.
- [ ] Implement GitHub App JWT generation and installation-token exchange with bounded HTTP timeouts and API-version headers.
- [ ] Implement Axum request limits, tracing correlation IDs, JSON error envelopes, and webhook signature verification over the exact raw bytes.
- [ ] Run `cargo test --test config --test github_auth --test webhook_http`.

### Task 3: Build the Postgres schema, event ledger, watchlist, policy, and readiness model

**Files:**
- Create: `migrations/0001_initial.sql`, `src/db.rs`, `src/models.rs`, `src/repository.rs`, `src/policy.rs`, `src/readiness.rs`
- Test: `tests/repository.rs`, `tests/readiness.rs`, `tests/policy.rs`, `tests/features/watchlist.feature`

**Interfaces:**
- `EventRepository::record_delivery(delivery: DeliveryEnvelope) -> Result<DeliveryOutcome>` is idempotent by GitHub delivery ID.
- `WatchlistRepository::upsert_pull_request(snapshot: PullRequestSnapshot) -> Result<WatchState>` and `WatchlistRepository::close_pull_request(repository_id, number, reason)` maintain lifecycle state.
- `Readiness::evaluate(input: ReadinessInput, policy: &Policy) -> ReadinessSnapshot` is deterministic and returns stable reason codes.
- `Policy::from_yaml(source: &str) -> Result<Policy, PolicyError>` validates `.pitools.yml` and applies immutable service guardrails.

- [ ] Write the SQL migration for installations, repositories, pull requests, event deliveries, feedback items, checks, policies, readiness snapshots, jobs, controls, comments, and audit entries with unique delivery and GitHub-object keys.
- [ ] Write repository tests for duplicate delivery, out-of-order close/open events, merged/closed cleanup, and transaction rollback.
- [ ] Write readiness tests for conflicts, stale branches, failed/pending checks, missing approvals, unresolved configured automation feedback, and a fully ready PR.
- [ ] Implement typed policy parsing for automation actors, required checks, branch freshness, approval requirements, force-push mode, and reconciliation interval.
- [ ] Implement the migration runner and repository transactions.
- [ ] Run the repository and readiness tests against a disposable Postgres test database.

### Task 4: Normalize GitHub events and implement reconciliation

**Files:**
- Create: `src/github/events.rs`, `src/github/sync.rs`, `src/jobs/reconcile.rs`, `src/queue.rs`
- Modify: `src/web.rs`, `src/main.rs`
- Test: `tests/events.rs`, `tests/reconciliation.rs`, `tests/features/webhook_watchlist.feature`

**Interfaces:**
- `EventNormalizer::normalize(event_name: &str, payload: &[u8]) -> Result<Vec<DomainEvent>>` accepts selected GitHub App events and records unsupported actions without guessing.
- `Reconciler::refresh_open_pull_requests(installation_id: i64) -> Result<ReconcileReport>` refreshes PR, review, thread, check, workflow, and status state.
- `JobQueue::enqueue(job: JobSpec) -> Result<JobId>` persists a job and publishes a best-effort NATS wakeup.

- [ ] Add fixtures for pull request opened/synchronize/closed/merged, issue comments, reviews, review comments, check runs, workflow runs, statuses, and requested actions.
- [ ] Implement strict event normalization with payload size limits and normalized actor/repository/PR references.
- [ ] Enqueue reconciliation after relevant webhooks and on a bounded periodic interval.
- [ ] Implement Postgres lease/heartbeat/retry/cancel state so jobs survive NATS loss and worker restarts.
- [ ] Add BDD scenarios for signature rejection, duplicate delivery, watchlist entry/exit, and missed-webhook repair.
- [ ] Run focused event, queue, and BDD checks.

### Task 5: Add API, CLI, policy loading, and operator observability

**Files:**
- Create: `src/api.rs`, `src/admin.rs`, `src/bin/pitools.rs`, `src/metrics.rs`
- Modify: `src/web.rs`, `src/main.rs`, `README.md`
- Test: `tests/api.rs`, `tests/cli.rs`, `tests/features/operator_surface.feature`

**Interfaces:**
- `GET /api/v1/watchlist` lists watched PRs with readiness and latest job state.
- `GET /api/v1/watchlist/:id`, `GET /api/v1/events`, and `GET /api/v1/audit` return redacted operational state.
- `POST /api/v1/reconcile` and `POST /api/v1/jobs/:id/cancel` require the admin bearer token.
- `pitools doctor`, `pitools watchlist`, `pitools events`, `pitools audit`, `pitools reconcile`, and `pitools cancel` use the configured durable operator state.

- [ ] Write API contract tests for authentication, pagination, redaction, readiness filters, and cancellation authorization.
- [ ] Implement bearer-token verification against a stored hash, never accepting the raw token in logs or database rows.
- [ ] Implement CLI output as stable human-readable text plus `--json` output.
- [ ] Add structured logs and Prometheus-compatible metrics for webhook verification, delivery outcomes, reconciliation, queue leases, and job controls.
- [ ] Run API, CLI, and BDD checks.

### Task 6: Implement living comments, final summaries, Check Run controls, and audit authorization

**Files:**
- Create: `src/github/comments.rs`, `src/github/check_runs.rs`, `src/jobs/controls.rs`, `src/jobs/progress.rs`
- Modify: `src/github/events.rs`, `src/models.rs`, `src/repository.rs`
- Test: `tests/comments.rs`, `tests/check_controls.rs`, `tests/features/job_controls.feature`

**Interfaces:**
- `ProgressReporter::start(job: JobContext)`, `ProgressReporter::update(job, phase, plan)`, and `ProgressReporter::finish(job, summary)` maintain one living comment and append one final summary.
- `CheckControlService::create_or_update(job) -> Result<ControlHandle>` creates a completed neutral Check Run with Skip and Cancel requested actions and a details link.
- `CheckControlService::authorize(actor, pull_request, action) -> Result<AuthorizedControl>` accepts only PR author or maintainer actors.
- `ControlState::apply(action) -> Result<ControlDecision>` marks one item skipped or the run cancelled and is checked before every mutation.

- [ ] Write tests for idempotent comment updates, comment marker isolation, escaped untrusted text, final-summary content, requested-action authorization, duplicate clicks, stale job controls, and cooperative cancellation.
- [ ] Implement GitHub Issues/PR comment and Checks API adapters with retry/backoff and bounded markdown rendering.
- [ ] Ensure the Check Run action identifiers are stable (`skip_item`, `cancel_run`) and that clicks cannot target another repository, PR, or job.
- [ ] Add BDD scenarios for start/update/final comment lifecycle and authorized/unauthorized controls.
- [ ] Run focused control and BDD checks against mocked GitHub responses.

### Task 7: Implement deterministic feedback repair

**Files:**
- Create: `src/feedback/classify.rs`, `src/feedback/suggestions.rs`, `src/feedback/repair.rs`, `src/worktree.rs`
- Test: `tests/feedback.rs`, `tests/suggestions.rs`, `tests/features/feedback_repair.feature`

**Interfaces:**
- `FeedbackClassifier::eligible(item, policy) -> Eligibility` accepts only configured automation actors.
- `SuggestionParser::extract(comment) -> Result<SuggestionPatch>` extracts exact GitHub suggested-change blocks and rejects ambiguous/malformed input.
- `RepairExecutor::apply_suggestion(job, patch) -> Result<RepairEvidence>` operates in an ephemeral worktree, runs configured checks, and returns a commit plan without direct GitHub writes.

- [ ] Add fixtures for valid, overlapping, stale, multi-file, malformed, and prompt-injection-shaped suggestions.
- [ ] Apply only exact patches under the repository root; reject path traversal, binary changes, oversized patches, and changes outside the comment scope.
- [ ] Run configured validation in a resource-limited worker and capture command, exit status, and redacted output.
- [ ] Require policy/approval before push; use the App bot identity and an audit trailer when committing.
- [ ] Update the living comment and final summary; resolve the source feedback only after push and validation succeed.
- [ ] Add BDD coverage for accept, reject, skip, cancel, validation failure, and stale feedback paths.

### Task 8: Implement isolated GitHub Actions repair and the Pi sidecar

**Files:**
- Create: `workers/pi/src/protocol.ts`, `workers/pi/src/main.ts`, `workers/pi/src/pi-runtime.ts`, `workers/pi/test/protocol.test.ts`, `workers/pi/test/redaction.test.ts`
- Create: `src/ci/actions.rs`, `src/pi_protocol.rs`, `src/jobs/ci_repair.rs`, `runner/Dockerfile`
- Test: `tests/ci_repair.rs`, `tests/pi_protocol.rs`, `tests/features/ci_repair.feature`

**Interfaces:**
- `PiJobRequest` includes job ID, repository snapshot, allowed paths, failure evidence, policy revision, and result schema version; it excludes GitHub secrets and bearer tokens.
- `PiJobResult` includes diagnosis, confidence, proposed file operations, validation commands, and unresolved risks; unknown fields and secret-like values are rejected or redacted.
- `CiRepairService::repair(failure: ActionFailure) -> Result<RepairEvidence>` only creates a mutation plan after deterministic failure normalization and explicit approval.

- [ ] Add fixtures for failed workflow runs, failed jobs, annotations, cancelled runs, external checks, and transient GitHub errors.
- [ ] Implement GitHub Actions-only normalization; track external providers as non-repairable readiness blockers.
- [ ] Implement the TypeScript sidecar with Pi SDK calls behind a provider interface, scrubbed child-process environment, egress allowlist configuration, output size limits, Valibot schema validation, and secret redaction.
- [ ] Implement the Rust wrapper with a versioned protocol, nonce/job binding, JSON schema checks, path allowlists, command allowlists, and fail-closed unknown-result handling.
- [ ] Package the sidecar and runner as ephemeral containers with no GitHub App private key, admin token, or repository secret mounts.
- [ ] Add BDD scenarios for diagnosis-only, approved repair, invalid result, secret detection, network denial, cancellation, and validation failure.

### Task 9: Implement stack management and safe rebasing

**Files:**
- Create: `src/stack.rs`, `src/jobs/stack_rebase.rs`, `src/github/branches.rs`
- Test: `tests/stack.rs`, `tests/stack_rebase.rs`, `tests/features/stack_management.feature`

**Interfaces:**
- `StackPlanner::plan(prs, policy) -> Result<StackPlan>` returns deterministic topological order, blockers, and required base updates.
- `RebasePlanner::plan(branch, target) -> Result<RebasePlan>` returns fast-forward/new-commit/rebase/conflict outcomes without mutating GitHub.
- `StackExecutor::apply(plan, approval) -> Result<StackEvidence>` refuses force-push unless the branch is explicitly bot-owned and policy-enabled.

- [ ] Add fixtures for explicit stack declarations, branch-base inference, cycles, missing parents, merged parents, conflicts, and out-of-date branches.
- [ ] Prefer fast-forward or new commits; require an explicit control/approval for history rewriting.
- [ ] Keep stack order and branch outcomes in the living comment and final summary.
- [ ] Add BDD coverage for clean stack, blocked stack, conflict, skip, cancel, and opted-in bot branch rewrite.

### Task 10: Package, deploy, document, and operate on dev01

**Files:**
- Create: `Dockerfile`, `docker-compose.local.yml`, `helm/pitools/Chart.yaml`, `helm/pitools/values.yaml`, `helm/pitools/templates/`
- Create: `docs/operations/`, `docs/deployment/dev01.md`, `.dockerignore`, `SECURITY.md`, `CONTRIBUTING.md`, `CHANGELOG.md`
- Modify: `README.md`, `Makefile`, `.github/workflows/build-image.yml`

**Interfaces:**
- The image runs the Rust service and TypeScript worker with separate processes and scrubbed environments.
- Helm values configure external Postgres, NATS, Envoy hostname/path, App secrets, admin token hash, resource limits, and worker egress policy without embedding secret values.
- Local Compose is for development/acceptance only; dev01 production integration is a Flux/HelmRelease change owned by the infrastructure repository.

- [ ] Build a minimal non-root runtime image and separate worker image with pinned base digests documented in the build workflow.
- [ ] Add Kubernetes probes, PodSecurity settings, NetworkPolicy, resource limits, ServiceAccount, and secret references.
- [ ] Add Helm tests/template validation and a dev01 runbook covering namespace, DNS/TLS route, App Manifest callback/webhook URL, external service endpoints, migrations, rollback, and authenticated UAT.
- [ ] Add security disclosure and contribution guidance, Apache license, and release notes.
- [ ] Run `make build`, Helm lint/template checks, image config checks, and the full local BDD suite.

### Task 11: Final validation and delivery handoff

**Files:**
- Modify: `CONTEXT.md`, `LEARNINGS.md`, `README.md`, release and workflow contracts as required by validation.

- [ ] Run the final changed-scope `make check` after all implementation changes.
- [ ] Run final non-test packaging/security checks through `make quality-gates` only when the post-merge CI environment owns that command; do not claim local post-merge evidence if it was not run.
- [ ] Perform an adversarial review for webhook replay, cross-PR controls, prompt injection, secret exfiltration, path traversal, job races, force-push authorization, and comment injection.
- [ ] Verify documentation matches actual commands and environment names without exposing credentials.
- [ ] Record exact validation commands, commit SHA, residual infrastructure prerequisites, and authenticated UAT gaps in `CONTEXT.md`.
- [ ] Create a coherent Conventional Commit sequence, push only after the user-created `Titanicar-US/PiTools` remote exists, and hand off merge/review authority to the user.
