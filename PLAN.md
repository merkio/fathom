# Fathom: Implementation Plan

A desktop database client (DBeaver-style) written in Rust, with LLM assistance.
Written to be executed step by step in Claude Code. Work one milestone at a time. Each milestone ends with a passing `cargo test` and `cargo clippy`.

## 1. Decisions

### Confirmed by you
| Area | Decision |
|---|---|
| UI | egui via `eframe` (pure Rust) |
| Databases, v1 | PostgreSQL, MySQL/MariaDB |
| Databases, later | ClickHouse, Oracle, Cassandra, MongoDB, etc. must be addable as a new crate plus one registration line |
| LLM providers | Anthropic (Claude), OpenAI (and OpenAI-compatible endpoints), Ollama |
| LLM features | Natural language to SQL; explain/optimize/fix SQL; chat about schema and data |
| Autocomplete | Both: schema-aware local completion (default) and LLM inline suggestions (opt-in) |

### Defaults I chose (change any you disagree with before M0)
| Area | Default | Why |
|---|---|---|
| Async runtime | `tokio` | Required by sqlx and reqwest |
| DB access | `sqlx` (Postgres, MySQL), runtime queries only | Async, pooled, introspection through `information_schema` |
| HTTP for LLMs | `reqwest` + `serde`, hand-written adapters (no vendor SDKs) | Small surface, one consistent streaming model |
| Local app storage | SQLite (`rusqlite`) for connections metadata, history, settings | Zero setup |
| Secrets | OS keychain via `keyring`; never stored in SQLite or logs | Security |
| SQL parsing | `sqlparser` crate | Read-only guard, statement splitting |
| Data sent to LLM | Schema metadata only (names, types, keys). Row data is opt-in per request | Privacy |
| LLM-generated SQL | Never auto-executed; the user must review and run it | Safety |
| Errors | `thiserror` in libraries, `anyhow` only in the composition root | Typed errors across layers |
| Async trait | `async-trait` and `futures::Stream` | Needed for `dyn` ports |

## 2. Architecture (Clean Architecture)

Dependencies point inward only: `ui` → `application` → `domain`. Adapters (infra) implement ports defined in `application`. Only the `app` binary knows concrete types.

```
fathom/
├── Cargo.toml                  # workspace
├── CLAUDE.md                   # rules for Claude Code
├── PLAN.md
└── crates/
    ├── domain/                 # entities & value objects. No I/O, no async, no framework deps
    ├── application/            # use cases + ports (traits) + DTOs + prompt building
    ├── db-postgres/            # adapter: implements DatabaseDriver
    ├── db-mysql/               # adapter: implements DatabaseDriver
    ├── llm-anthropic/          # adapter: implements LlmProvider
    ├── llm-openai/             # adapter (also OpenAI-compatible base_url)
    ├── llm-ollama/             # adapter
    ├── storage/                # adapters: SQLite repos, keyring SecretStore
    ├── ui-egui/                # presentation: views, presenters, widgets
    ├── app/                    # binary: composition root / DI, config, logging
    └── test-support/           # contract-test suites, fakes, testcontainers helpers
```

### Layer rules (enforced, see M0)
- `domain`: depends only on `serde`, `thiserror`, `uuid`. No `tokio`, `sqlx`, `reqwest`, `egui`.
- `application`: depends on `domain`, `async-trait`, `futures`, `sqlparser`. No concrete drivers or UI.
- Adapters: depend on `application` (for the ports) and `domain`. They never depend on each other.
- `ui-egui`: depends on `application` and `domain` only. It never imports `sqlx`, `reqwest` or any adapter.
- `app`: the only crate that depends on everything.

### Domain (crates/domain)
- `ConnectionProfile { id, name, driver_kind, host, port, database, username, secret_ref, options }` (no password field)
- `DriverKind` (newtype over a string id such as `"postgres"`, not a closed enum, so new DBs don't touch domain)
- Schema model: `Catalog → Schema → Table/View → Column`, plus `Index`, `ForeignKey`, `DataType`
- `QueryRequest`, `QueryResult { columns, rows, rows_affected, elapsed, truncated }`, `Value` (typed cell)
- `QueryLanguage` (`Sql`, and room for `Mql`, `Cql`) and `Dialect` capabilities (identifier quoting, limit syntax)
- LLM concepts: `ChatMessage`, `Role`, `ModelId`, `ProviderId`
- `DomainError`

### Ports (crates/application/src/ports)
```rust
#[async_trait] trait DatabaseDriver { fn kind(&self) -> DriverKind; fn dialect(&self) -> Dialect;
    async fn connect(&self, profile: &ConnectionProfile, secret: Option<SecretString>)
        -> Result<Box<dyn DbSession>, DbError>; }
#[async_trait] trait DbSession { async fn introspect(&self, scope: IntrospectScope) -> Result<SchemaTree, DbError>;
    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError>;
    async fn cancel(&self) -> Result<(), DbError>; async fn close(self: Box<Self>); }
#[async_trait] trait LlmProvider { fn id(&self) -> ProviderId;
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError>;
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    fn stream(&self, req: LlmRequest) -> BoxStream<'static, Result<LlmChunk, LlmError>>; }
trait ConnectionRepository, QueryHistoryRepository, SettingsRepository, SecretStore, Clock
```
`DriverRegistry` and `LlmRegistry` map ids to `Arc<dyn …>`. **Adding ClickHouse = new `db-clickhouse` crate implementing `DatabaseDriver` + one `registry.register(...)` line in `app`.** No other crate changes.

### Use cases (crates/application/src/usecases)
- Connections: `SaveConnection`, `ListConnections`, `DeleteConnection`, `TestConnection`, `OpenSession`, `CloseSession`
- Data: `BrowseSchema`, `ExecuteQuery` (row limit, timeout, cancel), `ExportResult` (CSV/JSON), `QueryHistory`
- LLM: `GenerateSql` (NL→SQL), `ExplainSql`, `OptimizeSql`, `FixSql` (given error), `ChatAboutSchema` (streaming, multi-turn)
- Autocomplete: `CompleteSql` (schema-aware, local, no LLM) and `SuggestInline` (LLM ghost-text, opt-in). Both return a common `Suggestion { range, text, kind, source }` so the editor treats them uniformly. See section 3a.
- Support: `SchemaContextBuilder` (prunes the schema to fit a token budget, ranks tables by relevance to the question), `PromptBuilder` (versioned templates per feature, dialect-aware), `SqlSafetyGuard` (classify statements as read/write/DDL via `sqlparser`; applies to LLM output)

### UI (crates/ui-egui)
- Pattern: each screen has a `State`, a `Presenter` (calls use cases) and a `View` (pure egui drawing). Views contain no business logic.
- Async bridge: a `tokio` runtime on a background thread. The UI sends `Command`s over a channel, and results/stream chunks come back as `Event`s. Call `ctx.request_repaint()` on every event. Never block the UI thread.
- Screens: connection manager, schema tree (left), tabbed SQL editor (centre) with result grid (virtualized, `egui_extras::TableBuilder`), history, AI side panel (Generate / Explain / Optimize / Fix / Chat), settings (providers, models, privacy toggles).

## 3. LLM design
- All providers produce the same normalized `LlmChunk` stream (text delta, usage, done). Provider quirks stay inside adapters.
- Anthropic: Messages API with SSE streaming. OpenAI: Chat Completions with SSE, configurable `base_url` to support compatible servers. Ollama: `/api/chat` with NDJSON streaming and `/api/tags` for model listing.
- API keys come from `SecretStore`, never from config files. Ollama needs no key.
- The prompt always contains: dialect, the pruned schema, the user task. SQL replies are requested inside a fenced block and parsed. Unparseable output is shown as text with a warning.
- Safety: generated SQL is parsed, classified (read/write/DDL) and shown with a badge. It lands in the editor and is never run automatically. Per-connection "read-only mode" blocks non-read statements in `ExecuteQuery`.
- Privacy: settings show exactly what is sent. Row data is excluded unless the user ticks "include sample rows" for that request.

## 3a. Autocomplete and suggestions

**Schema-aware autocomplete (default, local, no LLM)**
- `CompleteSql(sql, cursor, session_id) -> Vec<Suggestion>`, pure and synchronous over a cached `SchemaTree`. Fast enough to run on every keystroke, and sends nothing off the machine.
- Context detection from the text before the cursor (tokenized with `sqlparser`'s tokenizer, tolerant of incomplete SQL): after `FROM`/`JOIN` suggest tables and views; after `SELECT`/`WHERE`/`ON` suggest columns of the tables already in scope; after `alias.` suggest that table's columns; otherwise keywords and functions.
- Extras: join suggestions from foreign keys (`ON a.id = b.a_id`), alias resolution, ranking by recent use from query history.
- Dialect data (keywords, functions, quoting) comes from `Dialect`, so each new database adapter supplies its own list. No SQL-specific logic goes into the UI.
- The schema cache is refreshed after `BrowseSchema` and on a manual "Refresh" action, and invalidated after DDL statements.

**LLM inline suggestions (opt-in, off by default)**
- `SuggestInline(prefix, suffix, session_id) -> Stream<Suggestion>` shown as ghost text, accepted with Tab.
- Debounce (about 400 ms after the last keystroke), cancel stale requests when the user keeps typing, and cap `max_tokens`. Use a small, fast model chosen in settings (a local Ollama model is the recommended default).
- Prompt contains: dialect, schema pruned by `SchemaContextBuilder` to the tables referenced in the editor, text before and after the cursor. Schema only, no row data.
- A per-provider setting shows a clear warning that typing context is sent to that provider. Off by default and per-connection overridable.
- Suggestions pass through `SqlSafetyGuard` for labelling only. They never auto-run.

**UI (egui)**
- A custom SQL editor widget wraps `egui::TextEdit` to provide a completion popup (arrow keys, Enter/Tab to accept, Esc to close), ghost text rendering, and syntax highlighting. Plan time for this: egui has no built-in completion widget.
- Local suggestions take priority. LLM ghost text only appears when no popup is open.

## 4. Testing strategy
- `domain` and `application`: pure unit tests with fakes from `test-support` (fake driver, fake LLM, in-memory repos).
- Contract suites in `test-support`: one reusable test suite for `DatabaseDriver` and one for `LlmProvider`. Every adapter runs the same suite, so new adapters are verified for free.
- Postgres/MySQL: integration tests with `testcontainers`, behind a feature flag so plain `cargo test` stays fast.
- LLM adapters: `wiremock` recorded SSE/NDJSON fixtures. No real network in CI.
- Architecture test: a script/test that parses `cargo metadata` and fails if any crate depends on a forbidden one (see M0).
- UI: presenters are tested without egui. Views get light smoke tests only.

## 5. Milestones (each is one Claude Code session)

Each milestone: **Goal / Tasks / Done when**. Keep PRs small.

### M0. Workspace skeleton and guardrails
- Create the workspace and all crates (empty `lib.rs`), `rust-toolchain.toml`, `rustfmt.toml`, `clippy` config (`-D warnings`), `deny.toml`.
- Add `xtask` or a test `tests/architecture.rs` that reads `cargo metadata` and enforces the layer rules in section 2.
- CI: fmt, clippy, test, deny.
- **Done when:** adding `sqlx` to `domain` makes the architecture test fail.

### M1. Domain and ports
- Implement the domain types and all port traits with docs. Build `test-support` fakes.
- **Done when:** domain unit tests pass; the fake driver and fake LLM compile against the ports.

### M2. Connection management
- Use cases for connection CRUD and test; SQLite repo; keyring `SecretStore` (with an in-memory fallback for headless CI).
- **Done when:** use-case tests pass with fakes; storage integration tests pass; no password appears in the SQLite file.

### M3. PostgreSQL adapter
- `db-postgres`: connect, introspect (schemas, tables, columns, PKs, FKs, indexes), execute with limit, timeout and cancel, value mapping.
- Run the `DatabaseDriver` contract suite against a container.
- **Done when:** contract suite is green.

### M4. MySQL/MariaDB adapter
- Same as M3 for `db-mysql`. Extract shared helpers into `application` only if they are DB-agnostic.
- **Done when:** the same contract suite is green for MySQL and MariaDB.

### M5. egui shell and data browsing
- `app` composition root, runtime bridge, connection manager screen, schema tree, SQL editor tab, virtualized result grid, cancel button, history.
- **Done when:** manual smoke test: connect to Postgres and MySQL, browse, run a query, cancel a long one.

### M5b. Schema-aware autocomplete
- `CompleteSql` use case, SQL context detector, per-dialect keyword/function lists in `Dialect`, schema cache, FK join suggestions, history-based ranking.
- egui editor widget with completion popup and syntax highlighting.
- **Done when:** table-driven tests cover `FROM`, `JOIN ... ON`, `alias.`, incomplete SQL and quoted identifiers for Postgres and MySQL; popup works in the manual smoke test.

### M6. LLM core and adapters
- `LlmProvider` use cases, `SchemaContextBuilder`, `PromptBuilder`, `SqlSafetyGuard`.
- Adapters: `llm-anthropic`, `llm-openai`, `llm-ollama` with streaming, passing the `LlmProvider` contract suite on wiremock fixtures.
- **Done when:** the contract suite is green for all three; prompt snapshot tests (`insta`) cover each feature.

### M7. AI features in the UI
- AI side panel: Generate (NL→SQL), Explain, Optimize, Fix (one-click from an error), Chat. Streaming output, stop button, insert-into-editor, safety badge, privacy preview, provider/model picker, settings screen.
- **Done when:** end-to-end manual test against Ollama (local) and one cloud provider.

### M7b. LLM inline suggestions
- `SuggestInline` use case with debounce and cancellation of stale requests, ghost-text rendering, Tab to accept, settings toggle and privacy warning (off by default).
- **Done when:** tests prove stale requests are cancelled and that nothing is sent while the setting is off; manual test with Ollama.

### M8. Hardening
- Result export, TLS options (SSH tunnelling removed from scope), large-result handling, error UX, logging with secret redaction, packaging (Linux/macOS/Windows).
- **Extensibility proof:** add a stub `db-sqlite` (or ClickHouse) adapter touching only the new crate and one line in `app`. This validates the architecture.
- **Done when:** the stub adapter lands with no changes in `domain`, `application` or `ui-egui`.

## 6. Open items to decide later
1. Non-SQL databases (Mongo, Cassandra): the ports allow different `QueryLanguage` values, but the result grid and the SQL-safety guard assume SQL. Decide how to handle this before building the first such adapter.
2. Whether to support LLM tool-use/agentic query execution. It was not selected, so it is out of scope.
3. Telemetry: none planned.
4. Licence for the project: decided, Apache-2.0 (see `LICENSE`).

## 7. Using this plan in Claude Code
- Put `CLAUDE.md` and `PLAN.md` in the repo root. Claude Code loads `CLAUDE.md` automatically.
- Start each session with: *"Implement milestone M3 from PLAN.md. Follow CLAUDE.md. Show me the plan before editing."*
- Use plan mode for M0, M1 and M6, because they set patterns the rest reuse.
- Ask Claude Code to tick the milestone in a `PROGRESS.md` when done.
