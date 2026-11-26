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
- [x] M8. Hardening (export, error hints, redacting logs, TLS options, SQLite extensibility proof, release workflow and README; SSH tunnel SKIPPED by decision (needs a large SSH client dependency; revisit later); macOS/Windows builds untested)
