# DBiever: Rules for Claude Code

DBiever is a desktop database client in Rust (egui) with LLM assistance. The roadmap is in `PLAN.md`. Work on one milestone at a time.

## Architecture (non-negotiable)
Clean Architecture in a Cargo workspace. Dependencies point inward: `ui-egui` → `application` → `domain`. Adapters (`db-*`, `llm-*`, `storage`) implement ports defined in `application`. Only `crates/app` wires concrete types.

- `domain`: no I/O, no async runtime, no `sqlx`/`reqwest`/`egui`.
- `application`: use cases, ports (traits), prompt building. No concrete drivers, no UI.
- Adapters never depend on each other. `ui-egui` never imports an adapter.
- New database = new `db-<name>` crate implementing `DatabaseDriver` + one registration line in `app`. If you need to edit `domain`, `application` or `ui-egui` to add a database, stop and ask.
- New LLM provider = new `llm-<name>` crate implementing `LlmProvider` + one registration line.

## Rules
- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` before declaring work done.
- Errors: `thiserror` in library crates, `anyhow` only in `crates/app`. No `unwrap`/`expect` outside tests.
- Never block the egui thread. UI talks to use cases through the command/event channel bridge.
- Secrets (DB passwords, API keys) only through `SecretStore`. Never log or persist them. Use `secrecy::SecretString`.
- LLM-generated SQL is never executed automatically. It must go through `SqlSafetyGuard` and the user's explicit action.
- Local autocomplete (`CompleteSql`) must stay offline and synchronous. LLM inline suggestions are off by default and must send nothing while disabled.
- Send schema metadata to LLMs only. Row data only when the user opts in per request.
- Every new adapter must pass the shared contract suite in `crates/test-support`.
- Add tests with the code. Application tests use fakes, not real databases or networks.
- Prefer small, focused commits. Do not add dependencies without saying why.

## When unsure
Ask the user rather than guessing, especially for: public port signatures, new dependencies, schema or storage format changes, and anything affecting what data leaves the machine.
