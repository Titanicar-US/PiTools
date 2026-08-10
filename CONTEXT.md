# PiTools active repository state

- Repository: `Titanicar-US/PiTools`; this checkout is the initial bootstrap and has no commit or remote configured yet.
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
- Helm security rendering, local Compose validation with dummy non-production variables, the non-root core image build (`sha256:98a160382142b3ff3d9c79861ccc8d267ab85fb3c84132f17857fe3bf0c291`), and the corrected pinned-NATS request/reply smoke have passed. The core image reports `pitools 0.1.0`; the runner returns request-bound diagnosis-only JSON when no provider workflow is selected.
- The runner dependency audit reports 3 upstream transitive npm advisories (1 moderate, 2 high, 0 critical) that the dependency advisor did not authorize an override for; this remains a release review item.

## External release prerequisites

- This checkout is still uncommitted and has no configured remote, so no image publication, GitHub PR, or push has been performed. The current execution policy rejects approval-bound `git add`/commit mutations, so publication remains pending rather than being simulated.
- Live delivery still requires GitHub App credentials, a configured `Titanicar-US/PiTools` remote, dev01/Flux ownership, external secrets, database/NATS endpoints, image publication by digest, and authenticated webhook/PR UAT.
- The default Pi worker is diagnosis-only and no provider-backed run was performed here. Typed CI patch proposals and the approval-gated executor are implemented, but a live provider, repository allowlist, and fork-head write contract are intentionally not assumed.
- Stack planning, approved PR base-branch updates, leased branch-history rebases, bounded check-run output evidence, and low-cardinality Prometheus metrics are implemented. Full GitHub Actions log download/annotation repair remains a follow-up capability.
