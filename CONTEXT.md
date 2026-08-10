# PiTools active repository state

- Repository: [`Titanicar-US/PiTools`](https://github.com/Titanicar-US/PiTools); the public repository and draft PR [#1](https://github.com/Titanicar-US/PiTools/pull/1) are published. The PR head branch is `codex/bootstrap`; the PR records its current commit SHA.
- The Rust control plane owns GitHub authentication, durable PostgreSQL jobs, reconciliation, readiness, comments, Check Run controls, deterministic feedback mutation, typed CI patch validation/application, validation, branch pushes, and bounded operator API/CLI inspection.
- The production router exposes the verified webhook receiver only with application state; the unconfigured utility router no longer advertises a placeholder webhook endpoint.
- The TypeScript Pi worker runs separately over the bounded `pitools.pi.requests` NATS request/reply subject. It receives no GitHub App credentials and defaults to diagnosis-only output.
- Local deployment uses PostgreSQL, NATS, the HTTP server, a Rust worker, and an optional long-lived Pi worker. Helm values intentionally require external secrets and immutable image digests.
- The GitHub App Manifest callback exchanges the one-time GitHub code and returns credentials with `no-store`; PiTools does not persist them.

## Deliberate fail-closed boundaries

- Human merge/approval remains outside PiTools.
- CI Pi output is a bounded diagnosis/proposal; only exact allowlisted unified patches that declare and receive explicit Check Run approval, pass a fresh head read and configured validation, and record an auditable push may mutate a PR. Repository policy cannot disable this CI mutation gate.
- Approved feedback and CI repair pushes now re-read the exact remote branch head immediately before pushing, so branch movement after validation fails closed.
- Review-comment automation outcomes are replied to on the originating review comment before the thread is resolved; issue-comment outcomes are recorded in a bounded PR-level comment because no review thread exists. Deterministic rejection is persisted so the same suggestion is not retried indefinitely, while operational failures leave feedback open.
- Stack detection produces a deterministic order; declared stack base updates and branch rebases require explicit approval.
- Force-push is prohibited by default; a history rewrite is admitted only for an exact `bot_owned_branches` entry when `allow_bot_force_push` is true, and uses `--force-with-lease` against the fresh head.
- Fork-head repairs fail closed until a separately scoped head-repository push contract is added.

## Current validation evidence

- `make check` passed after the last implementation edit on 2026-08-10: `cargo fmt --all -- --check`, Clippy with warnings denied, the locked Rust test suite, the 21 BDD scenarios (21 passed, 62 steps passed), and the Pi worker check (17 tests passed). The focused contracts cover bounded Actions logs, check-run annotations, envelope redaction, oversize rejection, typed CI approval admission, protected operator routes, CLI command discovery, review-thread feedback replies, and issue-comment outcome rendering; waiting-approval Skip/Cancel behavior also passed the focused PR-control and PostgreSQL queue contracts.
- Code-bearing validation head `a13d7860bc4b5095e708e57716f46679c2d157d5` passed the [source workflow](https://github.com/Titanicar-US/PiTools/actions/runs/31385495832) and [core/runner image workflow](https://github.com/Titanicar-US/PiTools/actions/runs/31385495839). Pull-request image validation now uses the runner Docker CLI, so the hosted log warning scan was clean; the protected BuildKit publisher remains release-tag-only and was skipped as designed.
- Evidence-only documentation head `7dd8f54e720dea776adf00a117b3b58333b54627` passed the exact-head [source rerun](https://github.com/Titanicar-US/PiTools/actions/runs/31386232738) and [core/runner image rerun](https://github.com/Titanicar-US/PiTools/actions/runs/31386232736); both image builds and the clean warning scan remained green.
- The canonical release helper is confirmation-gated, accepts only `vMAJOR.MINOR.PATCH`, targets `Titanicar-US/PiTools` `main`, and its fake-`gh` contract test passed; no release was created during validation.
- The image workflow now runs its publication job only for a `v` tag ref and rejects non-semantic tag names before package credentials or image pushes are used.
- Helm render contracts now require an explicit Pi-worker provider HTTPS allowlist, keep provider SDK execution disabled by default, require a separate provider Secret and explicit key mappings when enabled, and reject application/GitHub/control-plane credential boundaries.
- `/readyz` now fails closed when NATS is absent or its bounded flush probe fails, in addition to the PostgreSQL connectivity check; it still does not claim GitHub or model-provider readiness.
- Ephemeral local Compose acceptance passed with non-production credentials: PostgreSQL and NATS became healthy, `/healthz` and `/readyz` returned success, unauthenticated operator access returned 401, a signed PR webhook returned 202, its duplicate returned 208, and watchlist/event persistence plus webhook metrics were observed. The unique Compose project and temporary credentials were removed after the run.
- Docker Compose 5.3.1 warns that local secret `uid`, `gid`, and `mode` settings are ignored; the local acceptance still passed because the generated source key remained readable to the test container, while production Helm security remains enforced separately.
- `make build` passed, and both local non-root images built successfully: core `sha256:d8c8713f14a2831b4039f546cb9fe5d4746a3db874f8aef00ad0ba1a35639ad6` and runner `sha256:b6519c75294ad64e009a217b5ecf9437b7c0f9b7cf78562bf008b35352c9b87c6`.
- Helm security rendering, local Compose validation with dummy non-production variables, the non-root core image build (`sha256:98a160382142b3ff3d9c79861ccc8d267ab85fb3c84132f17857fe3bf0c291`), and the corrected pinned-NATS request/reply smoke have passed. The core image reports `pitools 0.1.0`; the runner returns request-bound diagnosis-only JSON when no provider workflow is selected.
- The runner dependency audit reports 3 upstream transitive npm advisories (1 moderate, 2 high, 0 critical) that the dependency advisor did not authorize an override for; this remains a release review item.

## External release prerequisites

- The local checkout remains uncommitted because the execution policy rejects approval-bound `git add`/commit mutations. The authenticated GitHub fallback published the source tree and CI follow-up directly to the public remote; no local `git push` or merge was simulated.
- Live delivery still requires human merge of PR #1, GitHub App credentials, dev01/Flux ownership, external secrets, database/NATS endpoints, image publication by digest, and authenticated webhook/PR UAT.
- The default Pi worker is diagnosis-only and no provider-backed run was performed here. Typed CI patch proposals and the approval-gated executor are implemented, but a live provider, repository allowlist, and fork-head write contract are intentionally not assumed.
- Stack planning, approved PR base-branch updates, leased branch-history rebases, bounded check-run output plus Actions job-log/annotation evidence, and low-cardinality Prometheus metrics are implemented. Provider-backed repair, authenticated UAT, and full deployment evidence remain external prerequisites.