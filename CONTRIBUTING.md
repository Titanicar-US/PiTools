# Contributing

1. Read `AGENTS.md` and the repository design documents before changing behavior.
2. Keep deterministic logic in Rust. Treat Pi output as untrusted, bounded input.
3. Add executable BDD coverage for user-visible behavior and focused unit/contract tests for edge cases.
4. Run `make check` for changed-scope validation. Do not run `make check-full` or `make quality-gates` locally; post-merge CI owns those checks.
5. Never commit secrets, generated credentials, private keys, or provider payloads.
6. Do not merge, approve, force-push, or change infrastructure state from a development branch.

Use Conventional Commits and explain operational or migration impact in pull requests.
