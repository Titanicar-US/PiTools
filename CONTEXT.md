# PiTools active repository state

- Repository: [`Titanicar-US/PiTools`](https://github.com/Titanicar-US/PiTools); the public repository and draft PR [#1](https://github.com/Titanicar-US/PiTools/pull/1) are published. The PR head branch is `codex/bootstrap`; the PR records its current commit SHA.
- The Rust control plane owns GitHub authentication, durable PostgreSQL jobs, reconciliation, readiness, comments, Check Run controls, deterministic feedback mutation, typed CI patch validation/application, validation, and branch pushes.
- The TypeScript Pi worker runs separately over the bounded `pitools.pi.requests` NATS request/reply subject. It receives no GitHub App credentials and defaults to diagnosis-only output.
- Local deployment uses PostgreSQL, NATS, the HTTP server, a Rust worker, and an optional long-lived Pi worker. Helm values intentionally require external secrets and immutable image digests.
- The GitHub App Manifest callback exchanges the one-time GitHub code and returns credentials with `no-store`; PiTools does not persist them.

## Deliberate fail-closed boundaries

- Human merge/approval remains outside PiTools.
- CI Pi output is a bounded diagnosis/proposal; only exact allowlisted unified patches with explicit approval, a fresh head read, configured validation, and an auditable push may mutate a PR.
- Stack detection produces a deterministic order; declared stack base updates and branch rebases require explicit approval.
- Force-push is prohibited by default; a history rewrite is admitted only for an exact `bot_owned_branches` entry when `allow_bot_force_push` is true, and uses `--force-with-lease` against the fresh head.
- Fork-head repairs fail closed until a separately scoped head-repository push contract is added.

## Current validation evidence

- `make check` passed after the last implementation edit on 2026-08-10: `cargo fmt --all -- --check`, Clippy with warnings denied, the locked Rust test suite, the 14 BDD scenarios (14 passed, 41 steps passed), and the Pi worker check (16 tests passed). Waiting-approval Skip/Cancel behavior also passed the focused PR-control and PostgreSQL queue contracts.
- GitHub Actions PR checks for the published branch have passed after updating checkout/setup-node to Node 24-compatible pinned releases; the current status is tracked by PR #1 and no deprecation annotation remains.
- Helm security rendering, local Compose validation with dummy non-production variables, the non-root core image build (`sha256:98a160382142b3ff3d9c79861ccc8d267ab85fb3c84132f17857fe3bf0c291`), and the corrected pinned-NATS request/reply smoke have passed. The core image reports `pitools 0.1.0`; the runner returns request-bound diagnosis-only JSON when no provider workflow is selected.
- The runner dependency audit reports 3 upstream transitive npm advisories (1 moderate, 2 high, 0 critical) that the dependency advisor did not authorize an override for; this remains a release review item.

## External release prerequisites

- The local checkout remains uncommitted because the execution policy rejects approval-bound `git add`/commit mutations. The authenticated GitHub fallback published the source tree and CI follow-up directly to the public remote; no local `git push` or merge was simulated.
- Live delivery still requires human merge of PR #1, GitHub App credentials, dev01/Flux ownership, external secrets, database/NATS endpoints, image publication by digest, and authenticated webhook/PR UAT.
- The default Pi worker is diagnosis-only and no provider-backed run was performed here. Typed CI patch proposals and the approval-gated executor are implemented, but a live provider, repository allowlist, and fork-head write contract are intentionally not assumed.
- Stack planning, approved PR base-branch updates, leased branch-history rebases, bounded check-run output evidence, and low-cardinality Prometheus metrics are implemented. Full GitHub Actions log download/annotation repair remains a follow-up capability.
