# PiTools Flux Promotion Design

## Status

Approved by the project owner on 2026-08-13.

This design covers source-owned Flux promotion from PiTools to the
infrastructure repository `Titanicar-US/code_pipeline`. It does not authorize
the infrastructure-repository change, secret provisioning, Flux reconciliation,
or deployment.

## Goal

Make a released PiTools deployment candidate reproducibly promote into the
dev01 Flux repository without hand-copying manifests. PiTools publishes a
reviewable pull request containing only its activation files. A target-owned
GitHub Action accepts the generated PR only after structural and GitHub merge
gates pass, then enables GitHub-native auto-merge so the target repository's
branch protection remains authoritative.

The workflow must be idempotent: a source state that already matches the
activation repository creates no new commit or pull request. Every promoted
image reference is immutable and every promotion result remains separately
observable from Flux reconciliation and runtime UAT.

## Ownership and authorization boundaries

PiTools owns:

- the source Flux bundle and its manifest/security contracts;
- the source workflow that copies the bundle and opens or updates a target PR;
- validation that the target diff is confined to the PiTools activation paths;
- the source SHA, image digests, and workflow evidence recorded in the PR.

`code_pipeline` owns:

- the activation repository and its aggregate Kustomizations;
- the target-side PR validation and merge workflow;
- branch protection, required checks, review rules, and the final merge;
- Flux reconciliation after the merged commit reaches `main`.

The PiTools control plane never merges application PRs. This deployment
promotion workflow is a GitHub Actions release mechanism and is not an
application mutation path. It must not use the GitHub App private key,
webhook secret, admin token, model credential, or any application secret.

The target-side workflow is a separate infrastructure-repository change and
requires authorization from the `code_pipeline` owner. The PiTools workflow
requires a separately provisioned `CODE_PIPELINE_DEPLOY_TOKEN` with only the
target repository permissions needed to read `main`, push an automation branch,
and create or update a pull request. The token is never written to Git, logs,
PR bodies, or generated manifests.

## Source and target layout

PiTools will carry a target-shaped source bundle at:

```text
platform/targets/k8s-flux/apps/pitools/
  kustomization.yaml
  app.yaml
  route.yaml
  networkpolicy.yaml
```

The exact directory and filenames are copied to:

```text
platform/targets/k8s-flux/apps/pitools/
```

in `code_pipeline`. The target aggregate update is a separate, explicitly
allowlisted file:

```text
platform/targets/k8s-flux/apps/kustomization.yaml
```

The source bundle contains rendered Kubernetes resources following the
existing `code_pipeline` raw-Kustomize application convention. It contains no
Secret resource and no plaintext credential. Application, PostgreSQL, NATS,
and optional provider credentials remain references to externally managed
Secrets. Namespace, Gateway, hostname, TLS, DNS, egress, and Secret names must
be owner-confirmed before a deployable bundle is populated; observed cluster
conventions are not authorization.

The bundle must use the v0.1.2 paired immutable image references, or later
release-owned immutable digests. Mutable tags, image digests with the wrong
length, and unapproved image repositories are rejected. Provider-backed Pi
execution remains disabled unless separately authorized and configured.

## PiTools source workflow

Add a workflow dedicated to Flux promotion. It runs from protected `main`
when the source Flux bundle or workflow changes, and supports an explicitly
enabled manual dispatch for a release replay. It does not run from arbitrary
pull-request code and does not deploy to Kubernetes.

The workflow:

1. Checks out PiTools at the triggering commit.
2. Validates the complete source bundle: required files, Kustomize structure,
   immutable images, safe Secret references, no plaintext Secret documents,
   no `.env` or credential-like files, and the owner-approved deployment
   values.
3. Requires the protected promotion environment and
   `CODE_PIPELINE_DEPLOY_TOKEN`.
4. Checks out `Titanicar-US/code_pipeline` at `main`.
5. Copies only the source bundle into the identical target path, removing
   stale files only inside that bundle path.
6. Ensures the single PiTools app resource is present in the target apps
   aggregate without rewriting unrelated resources.
7. Fails if the resulting diff contains any path outside the exact activation
   allowlist.
8. If the target already matches, records an empty PR URL and succeeds.
9. Otherwise creates or updates a ready-for-review PR with a branch named
   `automation/pitools-flux-<run-id>-<run-attempt>`.

The PR title is:

```text
chore(deploy): sync PiTools Flux bundle
```

The body records the source repository and SHA, release/image digests,
workflow run URL, target paths, and a machine-readable PiTools promotion
marker. It contains no secret values. The workflow uses a bot identity and
`--force-with-lease` only for its own generated branch after rereading the
target head. It never force-pushes `main`.

## Target merge workflow

The `code_pipeline` change adds a target-owned workflow triggered by pull
request events against `main`. It does not check out or execute the pull
request head. All validation uses the event payload and GitHub API/CLI from
the workflow revision on the protected base branch.

The workflow considers a PR eligible only when all of the following hold:

- the base branch is exactly `main`;
- the head repository is `Titanicar-US/code_pipeline`, not a fork;
- the head branch exactly matches the generated PiTools automation pattern;
- the title and machine-readable body marker identify a PiTools promotion;
- the changed-file set is limited to the PiTools app directory and the apps
  aggregate;
- the PR is not closed, conflicted, or stale;
- all visible checks are in passing terminal states;
- GitHub reports the PR as eligible for merge under current repository policy.

After validation it enables GitHub-native auto-merge with a regular merge
commit and branch deletion. It does not use an administrative merge API that
bypasses protection. If review, required checks, or another branch rule is
still outstanding, the PR remains open and auto-merge waits. If structural
validation fails, the workflow fails closed and leaves the PR open for
inspection.

The target workflow must use explicit `contents: write` and
`pull-requests: write` permissions, avoid broad secrets, and never execute
untrusted PR scripts. It should be restricted to the generated branch and
marker rather than granting automatic merge behavior to ordinary PRs.

## Idempotency and concurrency

Promotion uses a deterministic source snapshot and a unique generated branch
per workflow attempt. Before committing, it compares only the activation
allowlist. Existing matching content is a successful no-op. A rerun updates
the same run-specific branch if it still exists; stale branches and PRs do not
authorize unrelated paths.

The source workflow uses a non-canceling concurrency group for the protected
branch. The target merge workflow is safe to receive duplicate `opened`,
`synchronize`, and `reopened` events: repeated validation and auto-merge
requests are harmless, and an already merged or closed PR is reported without
creating another mutation.

## Validation and evidence

Behavior-changing work includes an executable scenario under
`tests/features/` and focused contract tests. The tests cover:

- copying the source bundle into the identical target layout;
- preserving unrelated target files;
- inserting the app resource once and retaining existing aggregate ordering;
- no-op behavior when source and target match;
- rejection of mutable or malformed image references;
- rejection of Secret documents, plaintext credential material, and `.env`
  files;
- rejection of diffs outside the activation allowlist;
- deterministic branch, commit, title, and body metadata;
- failure-closed handling of target PR state and non-passing checks.

Local `make check` validates source contracts and Helm/security rendering.
The target repository must independently validate its merge workflow and
branch-policy behavior. A merged target commit, Flux source/Kustomization
readiness, workload health, signed GitHub App UAT, persistence/restart,
NATS-interruption, ingress, isolation, and redaction evidence remain separate
evidence tiers.

## Rollback and failure behavior

Any source validation, token, checkout, path, manifest, or target-state error
fails without pushing a branch or opening a PR when possible. If a PR already
exists, the workflow leaves it open and reports the failure; it does not close,
merge, or mutate unrelated branches. Deployment rollback is performed by the
infrastructure owner by reverting the target activation commit and
reconciling Flux. The source workflow has no Kubernetes or secret-manager
credentials.

