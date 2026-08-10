# Changelog

## Unreleased

- Bound Check Run requested-action controls to the persisted repository and pull request context, rejecting cross-PR or cross-repository control attempts before queue mutation.
- Added deterministic `PiTools-Job`, `PiTools-Repair`, and `PiTools-Head` trailers to feedback and CI repair commits for auditable bot mutations.
- Made `pitools doctor` follow GitHub App installation pagination links and sort reported account names deterministically.
- Made `make check` enforce Rust formatting and the fail-closed Helm security render contract.
- Bootstrapped the Rust GitHub App control plane, signed webhook receiver, durable Postgres watchlist, readiness evaluation, operator API, and CLI.
- Added living work-plan comments, immutable final summaries, and GitHub Check Run Skip/Cancel actions.
- Added deterministic feedback repair primitives, typed GitHub Actions/Pi boundaries, safe stack planning, queue fencing, and dev01-ready container/Helm packaging.
- Added the GitHub App Manifest conversion callback, bounded inline suggestion support, automatic linear-stack plan jobs, and a durable NATS-connected Pi worker deployment path.
- Added explicit Check Run plan approval, durable waiting-approval transitions, skip-item plan advancement, typed allowlisted CI patch application, and safe approved PR base-branch updates for declared stacks.
- Kept Skip and Cancel available during approval waits without implicitly authorizing the remaining plan.
- Added bounded GitHub check-run output evidence and redaction before Pi diagnosis, plus leased stack branch rebases with exact bot-owned branch authorization.
- Added bounded GitHub Actions job-log downloads and check-run annotations to the redacted Pi diagnosis envelope, with fail-closed response-size limits.
- Added low-cardinality Prometheus counters at the internal `/metrics` endpoint.
- Added bounded operator API reads for PR detail, webhook delivery metadata, and audit entries, plus matching CLI watchlist/events/audit/cancel commands.
- Hardened the release workflow with Node 24-compatible action pins and explicit immutable core/runner digest summaries for the dev01 Flux handoff.
- Added a confirmation-gated release helper that targets canonical `main` and drives the protected image publication workflow.
- Closed the Helm handoff gap by requiring an explicit, Pi-worker-only model-provider HTTPS egress allowlist in the deployment example and render contract.
- Added a fail-closed, separately secret-backed opt-in for provider SDK execution in the Pi worker.
- Hardened `/readyz` with a bounded NATS flush probe alongside the PostgreSQL check.
- Added an immediate remote-head lease check before approved feedback and CI repair pushes, rejecting branch movement after validation.
- Gated immutable image publication to exact `vMAJOR.MINOR.PATCH` tag refs, including manual workflow dispatches.
- Updated pinned GitHub Actions to current Node 24-compatible major releases after hosted CI reported the Node 20 deprecation warning.
- Removed the dead unconfigured-router webhook placeholder so webhook intake is exposed only through the stateful verified receiver.
- Added bounded applied/rejected replies to automation review comments and PR-level outcome comments for issue comments, with durable rejected state preventing repeat attempts.
- Hardened typed CI repair admission so provider patches cannot bypass explicit Check Run approval, even when repository policy disables general approval.
- Added fail-closed installation suspend/delete and repository-removal handling that stops reconciliation, hides revoked watchlist rows, and cancels active jobs.
- Unified the GitHub App manifest and scoped worker-token permission contract with `administration:read` for protected-branch requirement reads.
- Added a bounded, allowlisted source snapshot to the Pi request protocol; the sidecar receives source over validated NATS JSON and no longer relies on an empty shared workspace mount.
- Bound every approval to a visible `sha256:` proposal fingerprint and made queue approval fail closed when the stored proposal changes.
- Added protected-branch review-count, stale/latest-push, code-owner, and GitHub App-specific required-check readiness gates; policy checks are now additive.
- Partitioned unrelated open pull requests out of stack planning and delayed PR base-branch mutations until all planned branch safety checks complete.
- Added per-PR jobs, audit entries, and delivery history to the operator API/CLI, plus broader CI/Pi redaction for GitHub, cloud, package, JWT, PEM, and assignment-shaped secrets.
- Hardened repair workspaces with credential-free validation snapshots, safe Git config environment, and staged mode/rename/copy rejection.
- Fixed the operator watchlist query to qualify joined timestamp columns and added Postgres and BDD regression coverage for active watched pull requests.
- Made `pitools doctor` validate the GitHub App identity and report visible installations before deployment UAT.
