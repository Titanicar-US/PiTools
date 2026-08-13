# PiTools Flux Promotion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Add a reviewable, idempotent PiTools-to-code_pipeline Flux promotion path while keeping deployment and owner-authorized infrastructure changes separate.

**Architecture:** PiTools owns a digest-pinned source bundle and a protected GitHub Action that copies only the matching activation paths into a clean `code_pipeline` checkout and opens a ready-for-review PR. `code_pipeline` owns the target-side guarded merge workflow, which validates PR identity and scope from protected base code and requests GitHub-native auto-merge. Deployment, Flux reconciliation, and UAT remain separate evidence tiers.

**Tech Stack:** GitHub Actions, Python 3 standard library, Rust/Cucumber contract tests, raw Kubernetes Kustomize manifests, GitHub CLI/API.

## Global Constraints

- Never place GitHub App credentials, webhook secrets, admin tokens, model credentials, or deployment secrets in Git or logs.
- Use only paired immutable PiTools image digests; reject mutable tags and malformed digests.
- Stage only `platform/targets/k8s-flux/apps/pitools/` and `platform/targets/k8s-flux/apps/kustomization.yaml` in the target PR.
- Do not execute untrusted PR code or bypass GitHub branch protection.
- Obtain owner-confirmed namespace, Secret references, Gateway/hostname/TLS, egress, and UAT inputs before populating the deployable bundle.

> **Execution note:** Implement each task in order. For every behavior change, write the failing focused test or contract first, observe the failure, then implement the smallest change that makes it pass.

## Scope and boundaries

This plan implements the PiTools-side promotion mechanism and prepares the
separate target-side `code_pipeline` change in a local checkout. It does not
publish either change, provision secrets, alter Flux, reconcile Kubernetes, or
perform deployment/UAT. The production Flux bundle remains gated on the
owner-confirmed namespace, Secret references, Gateway/hostname/TLS ownership,
egress selectors, and image-release inputs recorded in
`docs/deployment/dev01.md`.

## Task 1: Add the source Flux bundle contract and synchronization helper

Files:

- Add `scripts/publish-flux.py`.
- Add `tests/flux_promotion_contract.sh`.
- Add `tests/features/flux_promotion.feature`.
- Extend `tests/bdd.rs` with a fixture-backed synchronization scenario.
- Add the contract to `Makefile` under `check`.

Tests first:

1. Add a contract fixture containing a valid Kustomize bundle with the paired
   digest-pinned core and runner images, a target directory containing an
   unrelated file, and an aggregate Kustomization.
2. Assert that validation accepts the fixture and synchronization copies only
   the bundle into the identical target path.
3. Assert that synchronization preserves unrelated files and adds the `pitools`
   resource exactly once.
4. Assert that a second synchronization is a no-op.
5. Add failing fixtures for mutable image tags, malformed digests, Secret
   documents, `.env`/credential-like files, symlinks, missing required files,
   invalid Kustomization resources, and an unsafe destination/aggregate path.
6. Add an executable Gherkin scenario proving the valid source bundle is
   copied into the matching activation layout while unrelated target content
   remains unchanged.

Implementation:

- Make the helper standard-library-only so local `make check` and GitHub
  Actions do not need a new runtime dependency.
- Expose `validate` and `sync` subcommands with explicit source, destination,
  and aggregate paths.
- Require the exact source bundle file set and reject symlinks or nested files
  outside the approved bundle.
- Validate Kubernetes document headers, Kustomization resources, the paired
  PiTools GHCR repositories, and 64-character SHA-256 digests.
- Reject Secret documents, `stringData`, PEM material, `.env` files, and
  credential-bearing literal values while permitting `secretKeyRef` and
  external Secret names.
- Copy only within the destination bundle path, remove stale files only there,
  and update the aggregate with a line-preserving exact-resource insertion.
- Fail if any requested path is outside the expected source/target layout.

## Task 2: Add the source GitHub Action that opens the activation PR

Files:

- Add `.github/workflows/publish-flux.yml`.
- Extend the source workflow contract tests if needed.
- Update `docs/deployment/README.md` and `docs/deployment/dev01.md` with the
  promotion workflow and required owner-managed secret/environment setup.

Tests first:

1. Extend the shell contract to inspect the workflow text for protected-main
   or explicitly enabled dispatch gating, pinned action references, the
   separate `CODE_PIPELINE_DEPLOY_TOKEN`, the exact activation allowlist, and
   no application-secret or Kubernetes credentials.
2. Assert that the workflow invokes the helper before checkout mutation and
   stages only the allowlisted paths.
3. Assert that the PR title, generated branch pattern, no-op behavior,
   `--force-with-lease`, source SHA, image digests, and workflow URL are all
   represented.

Implementation:

- Trigger on protected `main` changes to the source Flux bundle/workflow and
  on an explicit manual dispatch.
- Use a non-canceling concurrency group and a protected promotion environment.
- Check out PiTools at `github.sha`, validate the source bundle, then check out
  `Titanicar-US/code_pipeline` at `main` using only
  `CODE_PIPELINE_DEPLOY_TOKEN`.
- Run the helper against the exact target paths, reject out-of-scope diffs,
  and record an empty output for an already synchronized target.
- Create or edit a ready-for-review PR on
  `automation/pitools-flux-<run-id>-<attempt>` with a Conventional Commit,
  deterministic promotion marker, source SHA, paired image digests, and no
  secret material.
- Push only the generated branch with `--force-with-lease`; never mutate
  target `main`.

## Task 3: Prepare the target-owned guarded merge workflow

Checkout:

- Create a fresh bounded clone of `Titanicar-US/code_pipeline` under `/tmp`.
- Verify remote, branch, HEAD, and clean state before editing.
- Do not push or open a target PR without explicit infrastructure-owner
  authorization.

Files in the separate checkout:

- Add `.github/workflows/merge-pitools-flux.yml`.
- Add a focused target-side contract test in the repository’s existing test
  convention, or a standalone shell/Python contract if no test harness exists.
- Add a short deployment automation README entry if the target repository has
  an established workflow documentation location.

Tests first:

1. Test acceptance of the exact PiTools automation branch, title, marker, and
   base repository.
2. Test rejection of fork heads, ordinary branches, drafts, wrong bases,
   missing markers, and changed files outside the PiTools activation allowlist.
3. Test rejection of conflicts, closed PRs, missing head SHA, and failing or
   pending visible checks.
4. Test that the merge command requests GitHub-native auto-merge with the
   regular merge method, branch deletion, and no administrative bypass.

Implementation:

- Use `pull_request_target` so the workflow runs from protected target code and
  never checks out or executes the untrusted PR head.
- Grant only explicit `contents: write` and `pull-requests: write`
  permissions; use `GITHUB_TOKEN` unless target policy requires a narrowly
  scoped owner-provided token.
- Read PR metadata and changed files through the GitHub API/CLI.
- Require `main`, the same-repository head, the exact generated branch pattern,
  the machine-readable PiTools marker, and only the app directory plus apps
  aggregate in the changed-file set.
- Verify visible checks are passing and GitHub reports the PR as mergeable.
- Request `gh pr merge --auto --merge --delete-branch`, leaving GitHub branch
  protection, reviews, and required checks authoritative. Fail closed on any
  structural or API error and leave the PR open.
- Make repeated webhook events harmless for already merged/closed PRs.

## Task 4: Populate the deployable source bundle only after owner inputs

Do not perform this task from observed cluster conventions. Before adding
`platform/targets/k8s-flux/apps/pitools/`, obtain and record:

- target namespace and owner;
- external PostgreSQL and NATS Secret names/keys and endpoint ownership;
- application Secret name/keys;
- Gateway name/namespace, listener, hostname, DNS owner, and TLS owner;
- ingress, DNS, database, NATS, GitHub/proxy, and optional model-provider
  egress selectors or CIDRs;
- exact accepted v0.1.2 image digests and registry access;
- authenticated UAT repository and scope.

Tests first:

1. Add the real bundle only with external Secret references and no Secret
   resource.
2. Render/validate the bundle and assert digest pinning, route scope, network
   policy peers, worker isolation, and migration/runtime settings.
3. Run the source synchronization contract against the real bundle and review
   the exact target diff.

Implementation:

- Add the target-shaped app directory and source Kustomization.
- Use the existing raw-Kustomize target convention and preserve the chart’s
  fail-closed security contracts.
- Keep provider-backed Pi disabled unless separately authorized.
- Use only the paired v0.1.2 immutable digests from the release handoff.

## Task 5: Verify locally and hand off without external mutation

1. Run focused helper and workflow contract tests.
2. Run the BDD test suite, `KUBECONFIG=/dev/null make check`, and
   `git diff --check` from the isolated PiTools checkout.
3. Run target-side contract tests in the separate local checkout.
4. Review both diffs and verify no generated artifacts, secrets, credentials,
   or unrelated paths are included.
5. Report the two local commit-ready changes, exact tests, unresolved owner
   inputs, and the fact that no branch was pushed, PR opened, merge performed,
   Flux reconciled, or cluster state changed.
