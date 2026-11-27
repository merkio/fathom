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
- [ ] D2. Command palette (done: Ctrl+K, panel toggles, themes, table search, focus mode); Database instances and Add instance dialogs (pending).
- [x] D3. ClickHouse adapter (`db-clickhouse`, official `clickhouse` client over HTTP; one registration line in `app`). Verified locally only against an old ClickHouse 18.16 (connect, values, errors, limits, timeout, cancel); introspection, data-skipping indexes and rows-written counts run in CI against `latest` and 25.8 (LTS).
- [ ] D4. Row editing with "Review changes" (needs `apply_changes` on the driver ports).
- [ ] D5. Export destinations as config-enabled adapters (local, S3, SMB, NFS), storage connections, config docs.
