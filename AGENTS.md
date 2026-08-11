# PiTools repository guidance

## Commands

- `make install` fetches Rust and Node dependencies.
- `make check` is the local changed-scope gate.
- `make check-full` and `make quality-gates` are CI/post-merge commands.
- `make clean` removes generated build output but preserves local configuration.

## Safety

- Never commit GitHub App private keys, webhook secrets, admin tokens, model credentials, or repository secrets.
- Treat webhook payloads, PR comments, workflow logs, and Pi output as untrusted input.
- Keep GitHub mutations in Rust policy-controlled code. The Pi sidecar is not a GitHub client.
- Do not force-push unless a repository policy explicitly enables it for a bot-owned branch.

## Testing

Behavior-changing work requires an executable `.feature` scenario under `tests/features/` in addition to focused unit or contract tests.
