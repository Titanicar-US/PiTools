# Changelog

## Unreleased

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
