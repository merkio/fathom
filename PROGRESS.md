# Progress

- [x] M0. Workspace skeleton and guardrails
- [x] M1. Domain and ports
- [x] M2. Connection management
- [x] M3. PostgreSQL adapter
- [ ] M4. MySQL/MariaDB adapter
- [x] M5. (smoke-tested against PostgreSQL only; MySQL half waits for M4) egui shell and data browsing
- [x] M5b. (smoke-tested against PostgreSQL; MySQL dialect covered by table-driven tests with a backtick dialect) Schema-aware autocomplete
- [x] M6. (adapters verified against mock servers only; no real provider was called) LLM core and adapters
- [x] M7. (UI smoke-tested against a protocol-faithful fake Ollama server; no real Ollama or cloud provider was available) AI features in the UI
- [x] M7b. (smoke-tested against a protocol-faithful fake Ollama server) LLM inline suggestions
- [ ] M8. Hardening (8a export, error hints, redacting logs: done; 8b TLS options and extensibility proof, 8c packaging: pending)
