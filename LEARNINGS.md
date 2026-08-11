# PiTools repository lessons

- Keep the GitHub App private key, installation tokens, webhook secret, and admin hash in the Rust control plane only. The Pi image should contain neither the application Secret nor GitHub API egress.
- NATS is a best-effort wakeup and Pi transport; PostgreSQL remains the durable queue and replay source of truth.
- Every GitHub mutation must be tied to a fresh read, a bounded typed plan, repository policy, an auditable job, and a final PR summary. A Pi diagnosis alone is not a mutation authorization.
- Check Run approval transitions must be durable in Postgres; a control click should release the same job and update the same living comment rather than create a second run.
- Skip and Cancel controls must work on waiting-approval jobs; skipping persists the current item and leaves later work unapproved.
- CI repair proposals use exact repository-relative paths and single-file unified patches. Suggested validation command text from Pi is never executed; repository policy owns the argv allowlist.
- Skipping a work item must persist its item ID in the job plan so a retry advances instead of replaying the skipped mutation.
- GitHub suggestion blocks need inline path and line coordinates. Exact unified patches remain useful for path-qualified automation feedback, while ordinary inline suggestions must be applied only to the supplied line range.
- Dev01 values belong to the Flux infrastructure owner. This repository supplies the chart, immutable image contract, external-secret references, network-policy shape, and evidence runbook but does not mutate the cluster.
- Branch-history rebases must compare the remote head before and during publication; only exact `bot_owned_branches` entries may use `--force-with-lease`, and fork heads remain read-only.
- Check-run output is fetched only for bounded numeric check-run IDs, redacted before Pi transport, and treated as diagnosis evidence rather than approval to mutate.
- Prometheus output should remain low-cardinality and unauthenticated only when the deployment ingress keeps `/metrics` inside the monitoring network.
