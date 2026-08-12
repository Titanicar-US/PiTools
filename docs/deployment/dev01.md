# dev01 Flux handoff

This is an infrastructure-owner runbook. It does not authorize changes to dev01, the Flux repository, external services, DNS, TLS, GitHub, or secret stores. Replace every quoted angle-bracket placeholder with an owner-confirmed value; none is a real hostname, repository, namespace, credential, or endpoint.

## Inputs and ownership gates

Before opening an infrastructure change, record:

- the immutable PiTools image repository and digest produced by the release workflow;
- the immutable runner image repository and digest for the isolated NATS Pi worker;
- the Flux infrastructure repository and environment path that own dev01;
- the namespace selected by the dev01 platform owner;
- external PostgreSQL and NATS URLs, their owning teams, and their secret-manager references;
- the existing Gateway name/namespace, approved public hostname, DNS owner, and TLS certificate owner;
- ingress-controller, DNS, PostgreSQL, NATS, GitHub API, and model-provider egress selectors or stable CIDRs;
- the GitHub App owner and the approved callback and webhook URLs.

Do not continue if any owner or source is unknown. Do not copy secret values into Git, Helm values, command history, tickets, or this runbook.

## Observed dev01 platform facts

Read-only inspection of the current dev01 cluster and Flux objects on 2026-08-12 confirms the following non-secret routing facts:

- Flux tracks `https://github.com/Titanicar-US/code_pipeline.git` on `main`.
- The dev01 root Kustomization path is `./platform/targets/k8s-flux/clusters/dev01`.
- The repository's application aggregate is `platform/targets/k8s-flux/apps/kustomization.yaml`; read-only inspection on 2026-08-12 found no PiTools entry or PiTools overlay. The Flux owner must choose and wire the PiTools app path into the appropriate dev01 aggregate before rollout.
- The existing application namespace is `codex-specops`.
- The existing public Gateway is `codex-specops-public` in namespace `codex-specops`, with the `hooks-https` listener for `hooks.e164sip.com`.
- The namespace currently provides `postgres:5432` and `nats:4222` Services.
- No PiTools Kubernetes resources are currently deployed.

These are discovery results, not deployment authorization. The dev01 platform owner must still confirm the namespace, Gateway/hostname/TLS ownership, service and secret references, egress policy, and authenticated UAT scope before an infrastructure change is opened. Do not infer owner approval from the existence of these resources.

## Secret contract

Provision three externally managed Kubernetes Secrets through the infrastructure repository's established SOPS, External Secrets, or equivalent workflow:

| Purpose | Required keys |
| --- | --- |
| Application | `github-app-id`, `github-app-private-key.pem`, `github-webhook-secret`, `admin-bearer-token-hash` |
| PostgreSQL | `database-url` |
| NATS | `nats-url` |
| Pi provider (optional) | Provider-specific keys named by `piWorker.provider.secretEnv` |

The chart references these Secrets and never creates them. Restrict the GitHub private key to the core PiTools pod only and rotate it through the GitHub App and secret owner procedures. The Pi worker must not mount or receive the application Secret, a Kubernetes service-account token, or GitHub API egress.

The current runner has two modes: the supported dev01 mode is the long-lived, dependency-free NATS request/reply worker; stdin is reserved for local one-shot protocol checks. Enable `piWorker` only with an immutable runner digest and owner-approved NATS egress. The Rust worker remains the owner of durable queue state, GitHub credentials, mutations, and result validation.

## Flux values handoff

Use the infrastructure repository's existing source and HelmRelease conventions. A values fragment has this shape:

```yaml
image:
  repository: "<release-owned-image-repository>"
  digest: "sha256:<release-owned-image-digest>"

worker:
  enabled: true

piWorker:
  enabled: true
  provider:
    enabled: false
    existingSecret: "<separate-provider-secret-name>"
    secretEnv:
      - name: "<provider-api-key-environment-name>"
        key: "<provider-api-key-secret-key>"
  networkPolicy:
    egress:
      https:
        enabled: true
        to:
          - ipBlock:
              cidr: "<platform-approved-model-provider-cidr>"
        port: 443
  image:
    repository: "<release-owned-runner-image-repository>"
    digest: "sha256:<release-owned-runner-image-digest>"

applicationSecret:
  existingSecret: "<application-secret-name>"

externalServices:
  postgres:
    existingSecret: "<postgres-url-secret-name>"
    urlKey: database-url
    port: 5432
  nats:
    existingSecret: "<nats-url-secret-name>"
    urlKey: nats-url
    port: 4222

httpRoute:
  enabled: true
  parentRefs:
    - name: "<existing-gateway-name>"
      namespace: "<existing-gateway-namespace>"
  hostnames:
    - "<approved-public-hostname>"
  pathPrefix: /github/webhook
  manifestCallbackPath: /github/manifest/callback

networkPolicy:
  ingress:
    from:
      - namespaceSelector:
          matchLabels:
            "<gateway-namespace-label-key>": "<gateway-namespace-label-value>"
        podSelector:
          matchLabels:
            "<gateway-pod-label-key>": "<gateway-pod-label-value>"
  egress:
    dns:
      to:
        - namespaceSelector:
            matchLabels:
              "<dns-namespace-label-key>": "<dns-namespace-label-value>"
          podSelector:
            matchLabels:
              "<dns-pod-label-key>": "<dns-pod-label-value>"
    postgres:
      to:
        - ipBlock:
            cidr: "<platform-approved-postgres-cidr>"
    nats:
      to:
        - ipBlock:
            cidr: "<platform-approved-nats-cidr>"
    https:
      enabled: true
      to:
        - ipBlock:
            cidr: "<platform-approved-https-egress-cidr>"
```

Use namespace/pod selectors instead of CIDRs when PostgreSQL or NATS is in-cluster. Narrow core HTTPS egress to the platform's approved GitHub API/proxy ranges and Pi-worker HTTPS egress to the approved model-provider or proxy ranges; do not use `0.0.0.0/0` as a convenience default. The chart keeps Pi-worker HTTPS disabled unless this separate `piWorker.networkPolicy.egress.https` allowlist is explicitly supplied.

## Pre-reconciliation checks

1. Render the exact Flux candidate values with `helm lint` and `helm template`.
2. Confirm the rendered core Deployment uses an image digest, UID/GID 10001, a read-only root filesystem, no privilege escalation, dropped capabilities, and no service-account token mount.
3. Confirm all secret values are `secretKeyRef` or secret-volume references and that no Secret manifest is rendered.
4. Confirm the Rust worker Deployment has the application Secret and the Pi worker Deployment has only NATS configuration; the Pi worker must not render application credentials.
5. Confirm the NetworkPolicies include only owner-approved ingress and egress peers.
6. Confirm the HTTPRoute binds only `/github/webhook` and the exact `/github/manifest/callback` path to the existing Gateway and approved hostname.
7. Confirm PostgreSQL backup/restore ownership and NATS availability expectations before reconciliation.

## Image publication handoff

Run the protected `build-image` workflow from an approved `v*` tag whose value matches both package manifests. Its `publish-image` matrix publishes multi-architecture `pitools` and `pitools-runner` images to GHCR and records each immutable manifest digest in the corresponding GitHub Actions job summary. Copy those exact `ghcr.io/<owner>/<image>@sha256:<digest>` references into the Flux values change; do not use a mutable tag in dev01. The `validate-image` jobs build with `push: false` and are not image-publication evidence.

From a checkout of the merged `main` branch, after the post-merge `main` checks are green, release [v0.1.2](https://github.com/Titanicar-US/PiTools/releases/tag/v0.1.2) is the current deployable release. Its immutable image references are `ghcr.io/titanicar-us/pitools@sha256:50c8ae460d0f873f35138dbc7a04d00f9bf8465f434aebd7169410997a397c3d` and `ghcr.io/titanicar-us/pitools-runner@sha256:35d01c8332d0ea62b0df5dd67d6c782a09b7c4b1dbe3c775b78fa8391e391fe4`. Never promote the partial v0.1.1 publication; use the exact digests above in the Flux values change.

## Database migration gate

The server connects to PostgreSQL and runs the embedded SQLx migrations before it binds the HTTP listener. Before reconciliation, the application and database owners must review the exact migrations in the candidate image, confirm backup/restore readiness and compatibility with the previous image, and authorize this startup-time migration model. Keep the first rollout at one replica and record the migration identifier and outcome. The chart deliberately does not add a second migration Job or ad hoc SQL path.

## Reconcile and observe

The Flux owner submits and reconciles the infrastructure change through the existing protected workflow. Keep these evidence tiers separate:

1. source review and rendered-manifest validation;
2. image digest availability for every target architecture;
3. Flux source and HelmRelease reconciliation at the intended revision;
4. Deployment rollout, available replicas, Service endpoints, HTTPRoute acceptance, and NetworkPolicy presence;
5. pod log and event review with no secret values;
6. direct `/healthz` and `/readyz` checks through an authorized internal path;
7. public TLS and webhook-route checks through the approved hostname.

The current `/readyz` handler includes a PostgreSQL connectivity check and a bounded NATS flush check. It does not prove GitHub or model-provider connectivity, and it is not a substitute for database migration or durability evidence. Verify each remaining dependency independently.

## GitHub App configuration and authenticated UAT

After DNS and TLS are confirmed, the GitHub App owner sets the App Manifest callback URL to `/github/manifest/callback` and the webhook URL to `/github/webhook` using the approved hostname. Reconfirm webhook signature secrets after any rotation.

Authenticated UAT requires evidence for:

- GitHub delivers a signed test event and receives the expected application response;
- an installation limited to an approved test repository is visible to PiTools;
- PostgreSQL persistence survives a pod restart;
- NATS interruption does not lose durable job state;
- ingress outside the webhook route is rejected;
- the runner receives no GitHub App private key, webhook secret, admin token, Kubernetes token, or unrelated repository credentials;
- logs, events, and status output contain no credentials.

Packaging validation does not establish authenticated webhook UAT or live readiness. A healthy pod, accepted HTTPRoute, successful migration, or Flux reconciliation is not a substitute for the signed-delivery and repository-installation evidence above.

## Rollback

1. The Flux owner reverts the environment commit to the last known-good image digest and values.
2. Reconcile through the normal Flux workflow and verify the HelmRelease and Deployment reach the prior revision.
3. Do not roll back database state unless the database owner confirms the migration is backward-incompatible and executes an approved restore or down-migration plan.
4. Preserve pod events, redacted logs, image digests, Flux revisions, and migration evidence for the incident record.
5. Disable or redirect the GitHub App webhook only with the GitHub App owner's authorization.
