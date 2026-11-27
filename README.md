# Fathom

[![CI](https://github.com/merkio/fathom/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/merkio/fathom/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/merkio/fathom/graph/badge.svg)](https://codecov.io/gh/merkio/fathom)
[![Dependencies](https://deps.rs/repo/github/merkio/fathom/status.svg)](https://deps.rs/repo/github/merkio/fathom)
[![Release](https://img.shields.io/github/v/release/merkio/fathom?include_prereleases&sort=semver)](https://github.com/merkio/fathom/releases)
[![License](https://img.shields.io/github/license/merkio/fathom)](LICENSE)
[![Rust edition](https://img.shields.io/badge/rust-edition%202024-orange?logo=rust)](https://doc.rust-lang.org/edition-guide/rust-2024/)
[![Last commit](https://img.shields.io/github/last-commit/merkio/fathom)](https://github.com/merkio/fathom/commits/main)

A desktop database client written in Rust ([egui](https://github.com/emilk/egui)), with
optional LLM assistance. It connects to PostgreSQL, MySQL/MariaDB, ClickHouse and SQLite. New databases and LLM providers plug in as separate crates.

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

## Editing rows

Results of a plain `SELECT` from one table that has a primary key can be edited in the grid:

- Double-click a cell to change it (type `NULL` to set NULL). Primary key cells are read-only, because rows are identified by them.
- Tick rows and press **Delete rows** to mark them for deletion.
- Nothing touches the database until you press **Review & apply**, read the list of changes and confirm. The grid is then reloaded from the database.

Edits run as parameterised `UPDATE`/`DELETE` statements keyed by the full primary key. On PostgreSQL, MySQL/MariaDB and SQLite the whole batch runs in one transaction and is rolled back unless every change affected exactly one row. ClickHouse has no transactions: changes run one by one as mutations and a failure leaves the earlier ones applied (the review dialog says so). Read-only connections never allow edits. Joins, aliases, expressions, `DISTINCT`, grouping and views are not editable.

## Saved queries

**Ctrl+S** (or **Save** next to Run) keeps the query in the editor under a name. Saved queries are
listed at the bottom of the sidebar and in the command palette (`Ctrl+K`); opening one puts it in
its own tab on the connection it was written for (or the current one) and never runs it. A dot on
the tab marks unsaved changes, and Ctrl+S updates the saved query in place ("Save query as..." in
the palette makes a copy). Names are unique, ignoring case. They are stored in Fathom's local
SQLite file (the first release with saved queries upgrades that file in place; nothing is lost).

## Exporting results

**Export...** (or `Ctrl+Shift+E`) saves a result as a new file in one of your *export destinations*:
a local folder, Amazon S3 or an S3-compatible service, or an SMB/NFS share that your operating
system has mounted. Formats: CSV, JSON, SQL (`INSERT` statements, never run by Fathom) and
Parquet. You can export all rows (the query is run again, up to `max_rows`) or only the rows you
ticked. Existing files are never overwritten.

Destinations are not edited inside the app, so credentials never pass through it: they are set in
`fathom.toml` and environment variables. The **Storage connections** dialog lists them, lets you
test each one, and shows what is wrong with the file.

### Setting up destinations (`fathom.toml`)

1. Copy [`fathom.example.toml`](fathom.example.toml) to `fathom.toml` in Fathom's data directory
   (Linux `~/.local/share/fathom`, macOS `~/Library/Application Support/fathom`, Windows
   `%APPDATA%\fathom\data`), or point `FATHOM_CONFIG` at it. The example documents every setting.
2. List the adapters you want in `enabled`; only those are used (`local`, `s3`, `smb`, `nfs`).
3. Add one `[[export.destination]]` table per place you export to.
4. Credentials: write `{ env = "VAR" }` or `{ file = "/path" }` instead of the secret, or leave
   S3 keys out to use the standard AWS credential chain (environment variables, `~/.aws`, SSO, an
   instance role). Keys named `access_key_id`, `secret_access_key`, `session_token`, `password`
   and `token` are never logged or displayed.
5. Open **Storage connections** (command palette, `Ctrl+K`) and press **Test connection**.

A mistake in one destination never stops the app: that destination is skipped and the dialog
explains why. Without a file, exports go to a "Local exports" folder in your documents directory.

| Kind | Needs | Notes |
|---|---|---|
| `local` | `path` | Created if missing |
| `s3` | `bucket`, optional `region`, `prefix`, `endpoint`, `profile`, keys | Needs `s3:PutObject` and `s3:GetObject`; an `endpoint` switches to path-style addressing for MinIO, R2 and similar |
| `smb`, `nfs` | `path` (the mount point) | Mount the share with your OS first; Fathom refuses to write if it is not mounted |

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
| `FATHOM_CONFIG` | Path of the configuration file (default: `fathom.toml` in the data directory) |
| `FATHOM_DATA_DIR` | Where the SQLite settings/history file and the log live (default: the platform data directory) |
| `FATHOM_SECRET_STORE=memory` | Do not use the OS keychain; keep passwords in memory only |
| `FATHOM_LOG` | Log level or filter (default `info`), e.g. `debug` |
| `FATHOM_OLLAMA_URL` | Ollama address (default `http://localhost:11434`) |
| `FATHOM_OPENAI_BASE_URL` | Point the OpenAI provider at a compatible server (vLLM, LM Studio, ...) |

Exports are written to your Documents folder (relative file names) and never overwrite existing files.

### Connection options

Set in the connection dialog's **Advanced** box, one `key=value` per line.

| Database | Option | Meaning |
|---|---|---|
| PostgreSQL | `sslmode` | `disable`, `allow`, `prefer` (default), `require`, `verify-ca`, `verify-full` |
| PostgreSQL | `sslrootcert` | CA certificate file used to verify the server (needed for private CAs) |
| PostgreSQL | `sslcert`, `sslkey` | Client certificate and key (mutual TLS) |
| MySQL / MariaDB | `sslmode`, `sslrootcert`, `sslcert`, `sslkey` | Same meaning as for PostgreSQL (`verify-full` verifies the host name). Default `prefer` uses TLS when the server offers it. MySQL 8+ accounts using `caching_sha2_password` need TLS: with `sslmode=disable` the connection fails with an RSA error |
| ClickHouse | `secure=true` or `sslmode=require`/`verify-ca`/`verify-full` | Use HTTPS (port 8443 unless set). The certificate is always verified against the system's root store. Default: plain HTTP on port 8123 |
| SQLite | `mode=ro` | Open the file read-only |
| SQLite | `create=true` | Create the file if it does not exist (off: a typo is an error) |

For SQLite, put the file path in the **Database** field (Host and User are ignored).

ClickHouse is reached over its HTTP interface (port 8123). The user is `default` unless you set one. ClickHouse has no foreign keys, and `UPDATE`/`DELETE` run as background mutations that report no row count.

The **Read-only** checkbox blocks everything except plain reads before it reaches the database.
It is a syntactic check; for a hard guarantee also use a database user with read-only rights.

## Tests

```sh
cargo test --workspace          # fast; no database or network needed
```

Integration tests against a real PostgreSQL are behind a feature flag:

```sh
FATHOM_TEST_PG_HOST=127.0.0.1 FATHOM_TEST_PG_PASSWORD=... \
  cargo test -p db-postgres --features integration
# or let testcontainers start one (needs Docker):
cargo test -p db-postgres --features containers
```

MySQL/MariaDB: set `FATHOM_TEST_MYSQL_HOST` (and `_PORT`, `_USER`, `_PASSWORD`, `_DB`) and run
`cargo test -p db-mysql --features integration`.

ClickHouse: set `FATHOM_TEST_CLICKHOUSE_HOST` (and `_PORT`, `_USER`, `_PASSWORD`, `_DB`) and run
`cargo test -p db-clickhouse --features integration`. Use a recent server: indexes and the
rows-written header need ClickHouse 19+, and CI runs `latest` and the 25.8 LTS.

TLS tests additionally need `FATHOM_TEST_PG_TLS_CA` and `FATHOM_TEST_PG_TLS_OTHER_CA` (paths to a
CA that signed the server certificate, and to an unrelated CA); without them they skip.

## Architecture

Clean Architecture in a Cargo workspace; dependencies point inward and a test enforces it
(`crates/test-support/tests/architecture.rs`).

```
domain        entities, value objects (no I/O)
application   use cases and ports (traits), prompt building, SQL helpers
db-*          database adapters (postgres, mysql/mariadb, sqlite)   -> implement DatabaseDriver
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

## Quality gates (CI)

Every push to any branch and every pull request runs the `CI` workflow; the badge above reflects it.

| Job | What it checks |
|---|---|
| Format | `cargo fmt --check` |
| Lint | `cargo clippy --all-targets --all-features -D warnings` (no `unwrap`/`expect` outside tests) |
| Tests | `cargo test --workspace`, including the architecture test that enforces the layer rules |
| Coverage | `cargo llvm-cov` over the unit **and** database integration tests; uploaded to Codecov. egui drawing code and `main()` are excluded (not testable without a display) |
| Integration | The shared driver contract against real PostgreSQL 18 and 16, MySQL 8.4 and 9, MariaDB 11 |
| Dependencies | `cargo-deny`: security advisories, licences, banned and unknown sources (also weekly) |
| Build (macOS, Windows) | Informational until proven green |

Dependabot keeps the workflow actions and Rust dependencies current. To enable the coverage badge, add
the repository to [Codecov](https://codecov.io) and set the `CODECOV_TOKEN` secret; to make the checks
mandatory, require them in the branch protection rules of `main`.

## Releasing

1. Set the new version in `Cargo.toml` (`[workspace.package] version`) and merge it.
2. Push a tag `vX.Y.Z` (for example `v0.2.0`; a suffix such as `-rc.1` makes a pre-release), **or**
   open Actions > Release > *Run workflow* on `main` and type the tag: the workflow then creates
   the tag on that commit itself (no local checkout needed).
3. The `Release` workflow runs the full CI, checks that the tag equals the crate version, builds Linux
   (x86_64), macOS (Apple Silicon and Intel) and Windows (x86_64), and attaches the archives and
   `SHA256SUMS.txt` to a GitHub release with generated notes.

The binaries are **not code-signed**: macOS Gatekeeper and Windows SmartScreen will warn on first run.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
