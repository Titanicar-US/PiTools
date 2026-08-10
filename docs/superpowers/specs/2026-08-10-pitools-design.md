# PiTools Design

**Status:** Approved for implementation

## Goal

PiTools is a public Apache-2.0 self-hosted GitHub App for GitHub.com. It keeps installed repositories' pull requests observable and moving toward human-merge readiness, while preserving deterministic control of GitHub state and isolating Pi-based reasoning behind an explicit worker contract.

The initial release is delivered as a Rust control plane with a TypeScript Pi worker sidecar. The control plane is the source of truth for webhook events, watchlist state, readiness, policy, approvals, mutations, comments, and audit history.

## User-visible behavior

- An operator creates a GitHub App through the GitHub App Manifest flow, configures the App credentials and webhook secret, and installs it on selected repositories.
- PiTools receives PR lifecycle, comment/review, check, workflow, and status events. It verifies signatures, deduplicates deliveries, records the event, and adds or removes PRs from the watchlist.
- A periodic reconciler refreshes open PRs so missed or out-of-order webhooks do not leave the watchlist stale.
- The API and CLI show each watched PR, its advisory readiness state, reasons, latest event, job, and audit history.
- A failed GitHub Actions check contributes bounded check-run output, first-page annotations, and plain-text job logs to diagnosis when GitHub makes those records available; the evidence is redacted before it crosses into Pi.
- A job that plans or makes changes creates one living PiTools status comment, updates it through its lifecycle, and appends a final immutable summary containing the plan, decisions, commits, files, checks, and unresolved items.
- The living comment links to a PiTools Check Run. The Check Run exposes GitHub-native Skip current item and Cancel run actions. GitHub returns action clicks as `check_run.requested_action` events.
- Skip affects only the current planned item. Cancel stops new work cooperatively and prevents further mutations. Only the PR author or a repository maintainer may use either control.
- PiTools never merges a PR. GitHub branch protection and human approval remain authoritative.

## Architecture

### Rust control plane

The Rust service owns HTTP ingress, GitHub App authentication, webhook verification, event normalization, Postgres persistence, job leases, readiness calculation, policy enforcement, GitHub API writes, comment/check-run rendering, and audit records. The CLI offers the corresponding installation validation, watchlist inspection, reconciliation, job control, and diagnostics against the configured durable operator state.

Postgres is the durable source of truth for installations, repositories, pull requests, event deliveries, normalized feedback/check state, policies, jobs, controls, comments, readiness snapshots, and audit entries. NATS is used for best-effort job wakeups plus the restricted Pi request/reply transport. Job leases, retry state, cancellation, and recovery are stored in Postgres, so dev01's current non-persistent NATS deployment is sufficient.

### Pi worker boundary

The TypeScript sidecar uses the Pi SDK/workflow runtime for feedback reasoning and CI diagnosis. The Rust control plane collects bounded check-run output, first-page annotations, and plain-text Actions job logs, redacts the evidence, then sends a bounded, versioned request over an authenticated, network-restricted NATS request/reply subject. The sidecar returns a nonce/job-bound JSON result. It has provider credentials only inside the worker boundary, no GitHub App private key or admin token, scrubbed repository-tool environments, and provider/allowlisted egress. A deterministic wrapper validates the result schema, limits paths and output size, redacts secrets, rejects malformed or unsafe plans, and hands the result to Rust. Pi never writes to GitHub directly.

### Deployment

PiTools publishes a container image and reusable Helm chart. The production profile consumes external Postgres and NATS services and is integrated into dev01's existing Flux-managed Kubernetes configuration. A local profile bundles equivalent dependencies for development and acceptance tests. Envoy Gateway or an equivalent existing TLS reverse proxy exposes the webhook route; the PiTools container remains private.

## Security and authority

- The GitHub App requests the full future-facing permission set from v1, but runtime actions still require repository policy and job approval.
- Webhooks require `X-Hub-Signature-256` verification and `X-GitHub-Delivery` idempotency.
- The App's private key and webhook secret are never exposed to Pi, repository commands, comments, or logs.
- Repository policy is versioned in `.pitools.yml`. Service-level invariants cannot be weakened by that file: PiTools cannot merge, cannot export secrets, and does not force-push by default.
- Typed CI patches always require an explicit Check Run approval; repository policy cannot disable this mutation gate, and Pi rejects any patch result that does not declare approval.
- Feedback automation is limited to configured automation actors. Human feedback is recorded and surfaced but is not automatically changed.
- Suggested changes are applied only after exact extraction, isolated worktree validation, configured checks, and policy approval.
- Optional provider-backed CI/feedback repair executes in an ephemeral least-privileged worker with bounded resources and provider/allowlisted network access; diagnosis-only mode remains the default.
- Repair commits use the GitHub App bot identity and include a job/audit trailer.

## Delivery slices

1. **Watchlist foundation and control UX:** repository bootstrap, event ledger, watchlist, reconciliation, readiness, API/CLI, policy, comments, Check Run controls, audit, tests, and deployment packaging.
2. **Deterministic feedback repair:** configured automation actor classification, review-thread state, exact suggested-change extraction, isolated worktree application, validation, comments, and resolution.
3. **GitHub Actions repair:** failed workflow/job normalization, ephemeral repair runner, Pi diagnosis/typed result wrapper, approval flow, commit/push, and final summary.
4. **Stack and rebase management:** explicit stack model, deterministic ordering, fast-forward/rebase planning, conflict handling, and opt-in force-push only for approved bot-owned branches.

Each slice is independently testable and keeps the prior slice usable.

## Acceptance boundary

The first release is complete when a self-hosted operator can validate an App installation, receive and replay signed events, see all open PRs in the API/CLI, recover state through reconciliation, inspect advisory readiness reasons, and exercise the comment/Check Run job-control contract without allowing an unapproved worker to mutate GitHub. Later slices are not declared complete until their own deterministic tests, BDD scenarios, security checks, and isolated integration evidence exist.