# Progress

- [x] M0. Workspace skeleton and guardrails
- [x] M1. Domain and ports
- [x] M2. Connection management
- [x] M3. PostgreSQL adapter
- [x] M4. (contract suite green against a real MariaDB 10.11; MySQL 8 runs only in the CI job, not verified locally) MySQL/MariaDB adapter
- [x] M5. (smoke-tested in a real window against PostgreSQL, SQLite and MariaDB; MySQL 8 not run) egui shell and data browsing
- [x] M5b. (smoke-tested against PostgreSQL, SQLite and MariaDB, including backtick-dialect completion) Schema-aware autocomplete
- [x] M6. (adapters verified against mock servers only; no real provider was called) LLM core and adapters
- [x] M7. (UI smoke-tested against a protocol-faithful fake Ollama server; no real Ollama or cloud provider was available) AI features in the UI
- [x] M7b. (smoke-tested against a protocol-faithful fake Ollama server) LLM inline suggestions
- [x] M8. Hardening (export, error hints, redacting logs, TLS options, SQLite extensibility proof, release workflow and README; SSH tunnelling removed from scope by decision; macOS/Windows builds untested)

## Design refresh (from the Figma mock-up)
- [x] D1. Rename to Fathom; SSH tunnelling removed from scope; themes (light/dark/system, persisted), header with breadcrumb and layout toggles, restyled explorer/tabs/editor/results/status bar. Verified by running the real app under Xvfb in both themes.
- [x] D2. Command palette (Ctrl+K), Database instances dialog (per-instance explorer visibility, saved in settings; nothing hidden until chosen), Add instance form with Connection and TLS tabs and "Connect & choose databases".
- [x] D3. ClickHouse adapter (`db-clickhouse`, official `clickhouse` client over HTTP; one registration line in `app`). Verified locally only against an old ClickHouse 18.16 (connect, values, errors, limits, timeout, cancel); introspection, data-skipping indexes and rows-written counts run in CI against `latest` and 25.8 (LTS).
- [x] D4. Row editing with "Review changes": `DbSession::apply_changes` (default: unsupported), one shared parameterised statement builder in `application`, implemented for PostgreSQL, MySQL/MariaDB, SQLite (one transaction, rolled back unless exactly one row changed per edit) and ClickHouse (mutations, no transactions). Shared contract covers it for every adapter; PostgreSQL, MariaDB and SQLite verified locally, ClickHouse and MySQL 8/9 in CI.
- [x] D5. Export destinations as adapters enabled by `fathom.toml` (`export-local` for local/SMB/NFS, `export-s3` on the official AWS SDK), credentials from env vars, files or the AWS chain, Export dialog (CSV, JSON, SQL, Parquet; all or selected rows), Storage connections dialog, `fathom.example.toml` with setup guide (tested). SMB and NFS use shares the OS has mounted (no native client). S3 verified against the moto emulator, not real AWS; destinations are edited in the file, not in the app.
