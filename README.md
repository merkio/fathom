# DBiever

A desktop database client written in Rust ([egui](https://github.com/emilk/egui)), with
optional LLM assistance. It connects to PostgreSQL and SQLite today; MySQL/MariaDB is planned (see
`PLAN.md`). New databases and LLM providers plug in as separate crates.

> **Status:** under development. See `PROGRESS.md` for which milestones are done and what has
> been verified (and against what).

## Features

- Connection manager with passwords in the OS keychain, TLS options and per-connection read-only mode
- Schema browser, tabbed SQL editor with syntax highlighting and **offline, schema-aware autocomplete**
  (tables, columns, aliases, foreign-key join conditions, quoting per dialect)
- Virtualized result grid, cancel for long queries, query history, export to CSV or JSON
- **AI assistant** (Anthropic, OpenAI and OpenAI-compatible servers, Ollama): natural language to SQL,
  explain, optimize, fix from an error, chat about the schema. Optional inline (ghost text) suggestions
- Generated SQL is classified (read / write / DDL), shown for review, and **never run automatically**

## Privacy: what leaves your machine

| Feature | What is sent | Default |
|---|---|---|
| Database | Only to the database you configured | n/a |
| AI assistant | The SQL dialect, your schema (table and column names, types, keys; **no data**) and your text. "Show what is sent" displays the exact prompt | Used only when you click Ask |
| Result rows to the AI | At most 5 rows of the current result, **only if you tick the box for that request** (resets after each request) | Off |
| Inline suggestions | The text around your cursor and the pruned schema, after each pause in typing | **Off**; each connection can override |
| Passwords, API keys | Stored only in the OS keychain; never logged | n/a |

Ollama on `localhost` (the default) keeps everything on this machine. Logs never contain SQL text,
prompts, result data or secrets, and are scrubbed for secrets as a safety net.

## Build and run

Requires a recent stable Rust toolchain.

```sh
cargo run --release -p app
```

**Linux:** the window needs the X11/Wayland client libraries at runtime. On Debian/Ubuntu:
`sudo apt install libxkbcommon-x11-0 libgl1 libegl1`. Passwords use the Secret Service (GNOME
Keyring, KWallet); on machines without one the app keeps passwords **in memory only** and tells you.

**macOS / Windows:** build with the same command. These platforms have not been tested yet (see
*Verification* in `PROGRESS.md`); the release workflow and a CI job build them on GitHub.

## Configuration

Environment variables:

| Variable | Meaning |
|---|---|
| `DBIEVER_DATA_DIR` | Where the SQLite settings/history file and the log live (default: the platform data directory) |
| `DBIEVER_SECRET_STORE=memory` | Do not use the OS keychain; keep passwords in memory only |
| `DBIEVER_LOG` | Log level or filter (default `info`), e.g. `debug` |
| `DBIEVER_OLLAMA_URL` | Ollama address (default `http://localhost:11434`) |
| `DBIEVER_OPENAI_BASE_URL` | Point the OpenAI provider at a compatible server (vLLM, LM Studio, ...) |

Exports are written to your Documents folder (relative file names) and never overwrite existing files.

### Connection options

Set in the connection dialog's **Advanced** box, one `key=value` per line.

| Database | Option | Meaning |
|---|---|---|
| PostgreSQL | `sslmode` | `disable`, `allow`, `prefer` (default), `require`, `verify-ca`, `verify-full` |
| PostgreSQL | `sslrootcert` | CA certificate file used to verify the server (needed for private CAs) |
| PostgreSQL | `sslcert`, `sslkey` | Client certificate and key (mutual TLS) |
| SQLite | `mode=ro` | Open the file read-only |
| SQLite | `create=true` | Create the file if it does not exist (off: a typo is an error) |

For SQLite, put the file path in the **Database** field (Host and User are ignored).

The **Read-only** checkbox blocks everything except plain reads before it reaches the database.
It is a syntactic check; for a hard guarantee also use a database user with read-only rights.

## Tests

```sh
cargo test --workspace          # fast; no database or network needed
```

Integration tests against a real PostgreSQL are behind a feature flag:

```sh
DBIEVER_TEST_PG_HOST=127.0.0.1 DBIEVER_TEST_PG_PASSWORD=... \
  cargo test -p db-postgres --features integration
# or let testcontainers start one (needs Docker):
cargo test -p db-postgres --features containers
```

TLS tests additionally need `DBIEVER_TEST_PG_TLS_CA` and `DBIEVER_TEST_PG_TLS_OTHER_CA` (paths to a
CA that signed the server certificate, and to an unrelated CA); without them they skip.

## Architecture

Clean Architecture in a Cargo workspace; dependencies point inward and a test enforces it
(`crates/test-support/tests/architecture.rs`).

```
domain        entities, value objects (no I/O)
application   use cases and ports (traits), prompt building, SQL helpers
db-*          database adapters (postgres, sqlite)   -> implement DatabaseDriver
llm-*         LLM adapters (anthropic, openai, ollama) -> implement LlmProvider
storage       SQLite repositories, OS keychain
ui-egui       presentation: presenters, views, command/event bridge
app           composition root: the only crate that knows concrete types
test-support  fakes and the shared contract suites every adapter must pass
```

**Adding a database:** create `crates/db-<name>` implementing `DatabaseDriver`, run the shared
contract suite from its tests, add it to the workspace, and add one `drivers.register(...)` line in
`crates/app/src/main.rs`. Nothing in `domain`, `application` or `ui-egui` changes (the SQLite adapter
was added exactly this way). **Adding an LLM provider** is the same with `LlmProvider`.

## License

Not yet chosen.
