<div align="center">

# Table X

**A fast, cross-platform database client for developers.**

Windows · Linux · macOS · iOS · Android

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)
[![Tauri](https://img.shields.io/badge/tauri-2.11-24C8DB.svg)](https://tauri.app)

</div>

---

> [!WARNING]
> **Status: pre-release (v0.2.0).** Five drivers, the editor, the grid, tunnelling and
> the power features are built and tested — see [Roadmap](#roadmap) for exactly what
> works today. Windows and Linux installers are produced by CI but are **not code-signed
> yet**, so the first launch warns. macOS is not built by CI — signing it needs a paid
> Apple membership, and an unsigned `.dmg` is refused rather than warned about. Nothing
> here has been through a release anyone else has installed.

---

## Table of contents

- [What this is](#what-this-is)
- [Relationship to TablePro](#relationship-to-tablepro)
- [Why Tauri](#why-tauri)
- [Roadmap](#roadmap)
- [Architecture](#architecture)
- [Design decisions](#design-decisions)
- [Security model](#security-model)
- [Getting started](#getting-started)
- [Project structure](#project-structure)
- [Development workflow](#development-workflow)
- [Writing a driver](#writing-a-driver)
- [Platform support](#platform-support)
- [Releases and signing](#releases-and-signing)
- [Contributing](#contributing)
- [License](#license)

---

## What this is

Table X is an open-source database client aimed at developers who spend their day in
SQL: a fast editor, a schema browser that handles large catalogs, and a result grid you
can edit in place — across every major database, on every major platform.

The design goals, in priority order:

1. **Correctness over convenience.** A database client that silently rounds a monetary
   value or updates more rows than you edited is worse than no client at all. Where a
   shortcut would trade accuracy for ergonomics, this codebase takes the accurate path
   and documents why.
2. **Native speed.** Connection handling, decoding, and export run in Rust. The UI stays
   responsive on million-row tables because it never receives a million rows.
3. **Genuinely cross-platform.** Not "macOS plus a port". One codebase, five targets,
   equal support.
4. **Honest affordances.** The UI hides what a given database cannot do rather than
   offering buttons that fail.

## Relationship to TablePro

This project is inspired by [TablePro](https://github.com/TableProApp/TablePro), an
excellent native database client for macOS and iOS. TablePro is written in Swift and
SwiftUI, and its Linux port is still in development.

**Table X is an independent, clean-room reimplementation.** It shares no code with
TablePro. It was written from scratch in Rust and TypeScript, and is not affiliated with
or endorsed by the TablePro project or its authors.

The reason for a separate codebase rather than a port is structural: TablePro's UI layer
is SwiftUI, which does not exist on Windows, Linux, or Android. Its core packages
(`TableProCore`) declare `.macOS(.v14)` and `.iOS(.v17)` only. Reaching five platforms
means a new UI layer regardless of approach, so this project starts from a stack that
targets all five natively.

If you are on macOS or iOS today, **use TablePro** — it is mature, native, and excellent.
This project exists for everyone else.

> TablePro is licensed AGPLv3. Because Table X shares no code with it, it is not a
> derivative work and carries its own permissive license. No TablePro source was consulted
> while writing any implementation file here.

## Why Tauri

A database client is an unusual desktop app: the *backend* work (connection pooling, wire
protocols, type decoding, streaming exports) is heavy and systems-flavored, while the
*frontend* work (a code editor, a virtualized grid) is exactly what the web platform is
best at. Tauri splits along that seam.

| | Tauri (chosen) | Electron | Avalonia/.NET | Flutter |
|---|---|---|---|---|
| Installer size | ~10 MB | ~150 MB | ~40 MB | ~25 MB |
| DB driver ecosystem | Excellent (Rust) | Excellent (Node) | Excellent (ADO.NET) | Weak |
| SQL editor component | CodeMirror 6 | Monaco/CodeMirror | AvaloniaEdit | none mature |
| Desktop + mobile | Yes (v2) | Desktop only | Desktop only | Yes |
| Large-result decoding | Native | JS-bound | Native | FFI-bound |

The tradeoff Tauri makes is that each OS renders in its own webview (WebView2, WKWebView,
WebKitGTK), so rendering differences are real and must be tested per platform. That is
handled with a conservative build target and a CI matrix that builds every desktop OS
being shipped — Windows and Linux today, macOS once it can be signed.

## Roadmap

Milestone 1 ("core + power features") is the current target.

| Status | Area | Detail |
|:---:|---|---|
| ✅ | **Core type system** | Dynamic value model, error taxonomy, schema types, connection config. 26 unit tests. |
| ✅ | **Driver contract** | `Driver`/`Connection` traits, capability negotiation, driver registry. |
| ✅ | **App scaffold** | Tauri 2.11 + React 19 + Vite 8 + Tailwind 4, icons for all 5 platforms. |
| ✅ | **SQLite driver** | Dynamic decoding by declared type, catalog introspection, column provenance, guarded inline edits. 47 tests against a real engine. |
| ✅ | **PostgreSQL driver** | Exact `NUMERIC`, column provenance, `pg_catalog` introspection, TLS via rustls. 45 tests, 18 against a live server. |
| ✅ | **MySQL / MariaDB driver** | Exact `DECIMAL`, column provenance, `information_schema` introspection, TLS via rustls. 21 tests. |
| ✅ | **SQL Server driver** | Exact `DECIMAL`, `sys.*` introspection, escaped-literal edits. Read-only results — tiberius exposes no column provenance. 24 tests. |
| ✅ | **IPC + session registry** | 14 commands, per-session locking, atomic connection persistence, OS keychain. |
| ✅ | **Connection manager UI** | Per-driver forms, test-connection, colour tags, read-only flag. |
| ✅ | **Schema browser** | Lazily expanded object tree with per-node caching, a name filter, and the database and open object marked. |
| ✅ | **Table structure** | Columns, types, nullability, defaults, indexes and foreign keys, behind a Data/Structure toggle in the table's own tab. |
| ✅ | **SQL editor** | CodeMirror 6, schema-aware autocomplete, run-selection, error positioning. |
| ✅ | **Result grid** | Virtualized rows, inline editing, row insert and delete, sorting, filtering, undo/redo, optional row banding. |
| ✅ | **Transactions** | Begin, commit and roll back on the four engines that have them, each in its own spelling. The indicator follows a `BEGIN` typed into the editor, not only the buttons. |
| 🚧 | **Query cancellation** | SQLite and PostgreSQL can stop a running statement; the button is hidden on the three drivers that cannot yet. |
| ✅ | **SSH tunnels** | Password / private key / agent auth, multi-hop chains (ProxyJump), mandatory per-hop host key verification. Tested end to end against an in-process SSH server. |
| ✅ | **Query history** | Every run persisted with its timing and outcome, searchable across connections. 10 tests. |
| ✅ | **Appearance** | Eight themes plus follow-system, data and interface fonts, adjustable data size, row banding. 26 tests, including one that fails a theme missing a token. |
| ✅ | **Multi-tab workspace** | Query, table, notebook and panel tabs, each carrying its database; resizable editor/results split; tabs and their SQL restored on launch. |
| ✅ | **CSV/JSON/SQL export, SQL import** | Streaming on all five drivers. Table, database, and SQL-file restore, with progress and cancellation. |
| ✅ | **ClickHouse driver** | HTTP + JSONCompact, exact wide integers and `Decimal`. Read-only — no row-level `UPDATE`. 23 tests. |
| ✅ | **Typed cell editors** | JSON validated and pretty-printed in a panel, booleans as a three-state list, binary as a hex viewer. |
| ✅ | **Command palette** | Fuzzy search over every action, Ctrl+K. |
| ✅ | **SQL formatting** | Offline tokenizer/emitter; the token stream is preserved, which is what makes it safe to run afterwards. |
| ✅ | **Result filtering** | Per-column operators over the fetched page, comparing decimals as text so exactness survives. |
| ✅ | **Saved snippets** | Named statements, persisted atomically, searchable from the palette. |
| ✅ | **CSV import** | Chunked RFC 4180 reader, column mapping with a preview, per-type literals. Appends only. |
| ✅ | **Server activity** | Live sessions, blockers, and server counters on four engines; a session can be ended. |
| ✅ | **Visual EXPLAIN** | One parser per engine into a common plan tree; self-cost bars and flagged estimate misses. `ANALYZE` on PostgreSQL only, inside a rolled-back transaction. |
| ✅ | **ER diagram** | Deterministic layered layout computed in Rust, one bulk query per schema, pan and zoom. 11 layout tests. |
| ✅ | **Schema diff + migration** | Compares two schemas on any connected pair, generates ordered DDL in the target engine's dialect, marks every destructive statement. Runs nothing. 17 tests. |
| ✅ | **`tablex` CLI** | Same drivers, no window: `query`, `export`, `import`, `diff`, `tables`. `diff --exit-code` is a CI drift gate. 26 tests, 10 driving the real binary. |
| ✅ | **Notebooks** | Prose and queries in one saved document, with a markdown renderer that emits React elements rather than HTML. Results deliberately not persisted. 21 tests. |
| ✅ | **Result charts** | Bar, line, area and scatter in hand-drawn SVG — no charting dependency. Exact values read back from text, not from the plotted float. 17 tests. |
| ✅ | **Governance-lite** | Confirmation gate on destructive statements, driven by the colour tag; unbounded ones require typing the connection name. Exportable query-history audit trail. 13 tests. |
| ✅ | **MCP server** | `tablex mcp` — query, list, describe and explain for an agent, read-only by default, row caps enforced, every call audited. 11 tests over real stdio. |
| ✅ | **Correctness made visible** | Exact columns marked in the grid, a per-result panel explaining what is guaranteed, and "read-only" replaced by which of four reasons applies. |
| ✅ | **Privileges and roles** | Principals, grants, role inheritance, and SQL Server denials on four engines. Privilege names stay in the engine's own words. 14 tests. |
| 🚧 | **CI/CD + packaging** | GitHub Actions builds MSI/NSIS, .deb, .rpm and AppImage on every tag. **Code signing is not set up**; macOS and mobile bundles are not built — see [Releases and signing](#releases-and-signing). |

Deliberately **out of scope** for milestone 1, and tracked for later: the third-party
plugin system, AI chat and query assistance, MCP server integration, and settings sync.

### Why mobile is not simply another build target

Tauri v2 targets Android and iOS from the code already written, and that is a
real structural advantage over every desktop-only competitor. It is not, however,
a build command away.

Building for either requires the platform SDK — Android Studio and the NDK, or
Xcode and a provisioning profile — neither of which exists on the machine this
was developed on, so nothing here has been compiled for a phone or run on one.
Beyond compiling, the parts that would need designing rather than porting are
concrete: the OS keychain integration is per-platform and the mobile ones differ
from the desktop ones; a schema tree and a result grid are pointer-and-keyboard
designs that need rethinking for touch, not shrinking; and SSH tunnelling on iOS
runs into background-execution limits that have no desktop equivalent.

Worth doing, and worth doing as its own piece of work with a device in hand
rather than as an untested cross-compile.

### Why Oracle and MongoDB are not simply two more drivers

Both are wanted, and neither is blocked on writing a `Driver` impl. They are blocked
on things worth stating rather than discovering halfway through.

**Oracle** has no pure-Rust client. Every crate available (`oracle`, `sibyl`) wraps
OCI or ODPI-C, which needs Oracle Instant Client present at both build and run time.
That collides directly with design goal 3: CI would need Instant Client installed on
all five targets, and the packaged app would have to either bundle it — which its
licence governs — or refuse to start until the user installs it themselves. The
workable shape is an optional Cargo feature, off by default, with the driver registry
reporting Oracle as unavailable in a standard build. That is a deliberate decision
about distribution, not a missing afternoon of work.

**MongoDB**'s driver is pure Rust and straightforward. The difficulty is that this app
is SQL-shaped: the editor, autocomplete, formatter, `EXPLAIN`, schema diff, and
privileges all assume a statement written in SQL, and MongoDB's query language is
JSON. Supporting it honestly means the workspace learning to host a non-SQL query
mode — not a sixth `Driver` implementation pretending its find filters are statements.
The part that can be settled in advance already is: `tablex_core::documents` decides
how heterogeneous documents become columns and rows, keeps absent fields distinct from
null ones, and decodes extended JSON so an exact `$numberDecimal` stays exact.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Frontend — React 19 + TypeScript + Tailwind 4      (src/)      │
│                                                                 │
│   Connection    Schema      CodeMirror 6     TanStack Virtual   │
│    manager      browser     SQL editor        result grid       │
└───────────────────────────┬─────────────────────────────────────┘
                            │  Tauri IPC — typed commands,
                            │  narrow capability allowlist
┌───────────────────────────▼─────────────────────────────────────┐
│  App shell — Rust                            (src-tauri/)       │
│                                                                 │
│   IPC commands  ·  connection registry  ·  OS keychain          │
│   Thin by design: no database logic lives here.                 │
└───────────────────────────┬─────────────────────────────────────┘
                            │
        ┌───────────────────┼───────────────────┐
        │                   │                   │
┌───────▼─────────────────┐ │ ┌─────────────────▼───────────────┐
│ tablex-core             │ │ │ tablex-drivers                  │
│ (crates/tablex-core)    │◄──┤ (crates/tablex-drivers)         │
│                         │ │ │                                 │
│ • Value model           │ │ │ • PostgreSQL   • SQLite         │
│ • Driver/Connection     │ │ │ • MySQL/MariaDB                 │
│ • Schema description    │ │ │ • SQL Server   • ClickHouse     │
│ • Error taxonomy        │ │ │                                 │
│ • Driver registry       │ │ │ One Cargo feature per driver.   │
│                         │ │ │                                 │
│ No Tauri, no GUI, no DB.│ │ └─────────────────────────────────┘
└─────────────────────────┘ │
                            │ ┌─────────────────────────────────┐
                            └─┤ tablex-tunnel                   │
                              │ (crates/tablex-tunnel)          │
                              │                                 │
                              │ SSH local port forwarding.      │
                              │ Established before the driver   │
                              │ connects, so drivers know       │
                              │ nothing about SSH.              │
                              └─────────────────────────────────┘
```

**Why the core is a separate crate.** `tablex-core` depends on no GUI toolkit, no
database, and not on Tauri. That buys three things: drivers can be tested headlessly in
CI without launching a window; the same logic can back a future CLI or headless server
without a rewrite; and the compile-fail boundary makes it impossible to accidentally leak
UI concerns into query execution.

## Design decisions

These are the choices most likely to look surprising, with the reasoning attached.

### Exact numerics are strings, not floats

`Value::Numeric(String)` carries the database's own textual representation.

PostgreSQL's unbounded `NUMERIC` and MySQL's `NUMERIC(65,30)` both exceed every
fixed-width decimal type in Rust, including 128-bit ones. Decoding them through `f64`
would silently round. For a client people use to inspect financial data, silently
rounding is a correctness bug, not a rendering detail. Strings round-trip losslessly;
client-side arithmetic on them is a deliberate non-goal.

### Unknown types degrade instead of failing

`Value::Unsupported { type_name, raw }` catches anything a driver cannot interpret —
PostGIS geometry, custom enums, range types, vendor extensions.

The alternative, failing the query, means one exotic column blanks out an entire result
set. Showing `POINT(1 2)` with an accurate type label is far more useful than showing an
error, and it means the app degrades gracefully against databases and extensions that
did not exist when it was compiled.

### Editability is computed, never assumed

A result set is editable only when **every** column resolves to the same source table
**and** a usable key exists. `ResultSet::recompute_editable()` enforces both.

Joins, aggregates, and computed columns are read-only as a result. This is not a
limitation to work around — a join has no single target for an `UPDATE`, so any edit
would be a guess. Similarly, `TableDetail::edit_key()` will not fall back to a *nullable*
unique index, because `NULL != NULL` means such a column cannot address exactly one row.

### Capabilities default to "unsupported"

`Capabilities::default()` advertises nothing. Every driver must opt into transactions,
cancellation, provenance, and streaming individually.

A half-finished driver that forgets to set a flag ends up with a UI that hides the
feature — annoying but safe. The inverse default would produce buttons that fail at
runtime.

### Errors carry categories, not just strings

`Error::category()` and `Error::is_retryable()` let the UI react to *kinds* of failure
without pattern-matching vendor error text. `Error::Query` additionally carries a
character position and a vendor code (`SQLSTATE`), so the editor can underline the
offending token rather than just printing a message.

Auth failures are explicitly **not** retryable: retrying identical bad credentials just
burns login attempts against a lockout policy.

### Fetches are capped by default

`FetchOptions::default()` caps at 1,000 rows with a 60-second timeout. `max_rows: None`
exists but is used only by exports, never by the grid. `ResultSet::truncated` tells the
UI to show a "load more" affordance instead of implying completeness.

A careless `SELECT * FROM events` against a production table should not be able to
exhaust the client's memory.

## Security model

| Concern | Approach |
|---|---|
| **Credentials at rest** | Never written to config. Stored in the OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service) via the `keyring` crate, looked up by connection id at connect time. `ConnectionConfig` has no password field, and a unit test asserts the serialized form contains no secret. |
| **Database vs. SSH secrets** | Stored under distinct keychain entries so a database password and a key passphrase cannot overwrite each other. |
| **Frontend privileges** | The Tauri capability allowlist (`src-tauri/capabilities/default.json`) grants the webview no filesystem, shell, or network access of its own. Every privileged action goes through a reviewed Rust command. |
| **CSP** | `script-src 'self'` with no `unsafe-eval`, enforced in both `index.html` and `tauri.conf.json`. |
| **SSH host keys** | Verified on every connect against a stored fingerprint. There is **no trust-on-first-use**: a tunnel opens only once the fingerprint is known, so the user must first call `ssh_host_fingerprint` and confirm what the server presented. An unverified tunnel offers no protection against exactly the attacker a bastion exists to defend against. |
| **SSH key material** | With agent authentication, no private key is ever loaded into this process — signing is delegated to the OS agent over a Unix socket or the Windows OpenSSH named pipe. |
| **Accidental writes** | Connections carry a `read_only` flag enforced client-side regardless of database permissions, plus a color tag so production connections are visibly distinct. |
| **Inline edits** | Built as parameterized `UPDATE`s keyed on the row's original values, and rolled back unless exactly one row is affected. |
| **Query history** | Plain text in the config directory, so it stays inspectable and greppable. Statements that assign a credential (`... PASSWORD 'x'`, `IDENTIFIED BY 'x'`) are dropped rather than redacted — a redaction that misses one vendor's syntax leaks the secret anyway — and the UI says so, so their absence is not a mystery. |

## Getting started

### Prerequisites

| Tool | Version | Notes |
|---|---|---|
| **Rust** | 1.82+ | [rustup](https://rustup.rs) strongly recommended (see note below) |
| **Node.js** | 20+ | |
| **pnpm** | 9+ | `npm install -g pnpm` |

Plus the platform toolchain Tauri needs:

<details>
<summary><b>Windows</b></summary>

- [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with the
  **Desktop development with C++** workload (supplies the MSVC linker).
- [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) — preinstalled
  on Windows 11 and current Windows 10.

</details>

<details>
<summary><b>Linux</b></summary>

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

</details>

<details>
<summary><b>macOS</b></summary>

```bash
xcode-select --install
```

</details>

> [!NOTE]
> **Install Rust via rustup.** If you used the standalone MSVC installer, `rustup` will not
> be on your PATH and you cannot add cross-compilation targets — which blocks the iOS,
> Android, and Linux ARM builds. Uninstall the standalone package and install from
> [rustup.rs](https://rustup.rs) before working on mobile targets.

### Run it

```bash
git clone https://github.com/PhillipMwaniki/table-x
cd table-x

pnpm install          # frontend dependencies
pnpm app:dev          # build Rust, start Vite, open the app
```

The first Rust build compiles the full dependency tree and takes several minutes.
Subsequent builds are incremental and fast.

### Build a release bundle

```bash
pnpm app:build
```

Artifacts land in `target/release/bundle/` — `.msi` and `.exe` on Windows,
`.deb`/`.rpm`/`.AppImage` on Linux, `.dmg` and `.app` on macOS. Building on a Mac works
regardless of what CI does; it is distribution, not compilation, that needs the Apple
membership.

## Project structure

```
table-x/
├── crates/
│   ├── tablex-core/           # Database-agnostic foundations. No Tauri, no GUI.
│   │   └── src/
│   │       ├── value.rs         #   Dynamic value model  (Value, ValueKind)
│   │       ├── driver.rs        #   Driver / Connection traits, Capabilities
│   │       ├── result.rs        #   ResultSet, Column, editability rules
│   │       ├── schema.rs        #   Object tree, TableDetail, edit-key selection
│   │       ├── config.rs        #   ConnectionConfig, TLS and SSH settings
│   │       ├── error.rs         #   Error taxonomy and wire payload
│   │       └── registry.rs      #   Driver registry
│   │
│   ├── tablex-drivers/        # One module per database, one Cargo feature each.
│   │   └── src/
│   │
│   └── tablex-tunnel/         # SSH local port forwarding.
│       └── src/
│
├── src-tauri/                   # Thin Tauri shell.
│   ├── src/
│   │   ├── main.rs              #   Entry point
│   │   ├── lib.rs               #   Builder, plugins, command registration
│   │   ├── ipc.rs               #   IPC command surface
│   │   └── state.rs             #   Process-wide state
│   ├── capabilities/            #   Permission allowlist (deliberately narrow)
│   ├── icons/                   #   Generated — do not edit by hand
│   └── tauri.conf.json
│
├── src/                         # React frontend.
│   ├── main.tsx
│   ├── App.tsx
│   └── styles.css               #   Design tokens, light + dark
│
├── scripts/
│   └── gen_icon.py              # Regenerates the source app icon (stdlib only)
│
├── Cargo.toml                   # Rust workspace root
├── package.json
└── vite.config.ts
```

## Development workflow

```bash
# Frontend
pnpm dev                  # Vite dev server alone (no Rust)
pnpm typecheck            # tsc --noEmit
pnpm fmt                  # prettier

# Rust
cargo test --workspace    # all tests
cargo test -p tablex-core
cargo clippy --workspace --all-targets
cargo fmt --all

# Full app
pnpm app:dev              # Rust + Vite + window, with hot reload on both sides
pnpm app:build            # release bundle
```

CI runs exactly these on every push: `pnpm typecheck`, `pnpm fmt:check`, `pnpm lint`,
`pnpm test`, `pnpm build`, then `cargo fmt --all --check`, `cargo clippy --workspace
--all-features --all-targets` with warnings denied, and `cargo test --workspace
--all-features`. Running them locally before pushing is the whole of the pre-flight.

Formatting is Prettier for TypeScript and CSS, rustfmt for Rust — both checked, so a
disagreement about layout is settled by a tool rather than in review. Markdown is
excluded from Prettier deliberately: it reflows prose, and this file's wrapping is
meant.

Linting is ESLint with `typescript-eslint` and the React hooks rules, scoped to `src/`.
**Errors fail CI; warnings do not.** There are sixteen warnings today — ten of them
`react-hooks/set-state-in-effect`, which flags working but extra-render-costing effect
patterns that want a considered rewrite rather than a rushed one. They are left visible
rather than silenced, and `pnpm lint` should move to `--max-warnings=0` once the list is
empty.

### Database tests

SQLite tests run everywhere with no setup — the engine is compiled in, and every
test uses a real in-memory database rather than a mock.

PostgreSQL splits in two. The decoding logic, where the subtle bugs live, is
covered by unit tests that need no server and always run. The integration tests
need a live server and are **skipped** unless `TABLEX_TEST_PG` is set:

```bash
# macOS / Linux
TABLEX_TEST_PG=postgres://user:password@localhost:5432/postgres cargo test -p tablex-drivers

# Windows PowerShell
$env:TABLEX_TEST_PG = "postgres://user:password@localhost:5432/postgres"
cargo test -p tablex-drivers
```

They are skipped rather than failed when unset, because a missing database is a
missing environment, not a broken driver — and a suite that fails on a fresh
checkout trains people to ignore it. The tradeoff is that a skipped test still
reports `ok`, so when you need to confirm they really ran, check for skips
explicitly:

```bash
cargo test -p tablex-drivers postgres::tests:: -- --nocapture | grep skipping
```

These tests create and drop tables prefixed `tx_`. Point them at a scratch
database, not one you care about.

### Logging

Set `TABLEX_LOG` using `tracing-subscriber` filter syntax:

```bash
TABLEX_LOG=tablex=debug pnpm app:dev     # macOS / Linux
$env:TABLEX_LOG="tablex=debug"; pnpm app:dev   # Windows PowerShell
```

### Regenerating icons

```bash
python scripts/gen_icon.py app-icon.png
pnpm tauri icon app-icon.png
```

## Writing a driver

A driver implements two traits from `tablex-core`. `Driver` is a stateless factory;
`Connection` is one live session.

```rust
use async_trait::async_trait;
use tablex_core::{
    config::ConnectionConfig,
    driver::{Capabilities, Connection, Driver, DriverInfo, FetchOptions, RowEdit},
    error::Result,
    result::QueryOutcome,
    schema::{SchemaNode, TableDetail},
};

pub struct MyDriver;

#[async_trait]
impl Driver for MyDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mydb".into(),
            name: "MyDB".into(),
            default_port: Some(1234),
            file_based: false,
            // Start from the default (nothing supported) and opt in only to what
            // is actually implemented. See "Capabilities default to unsupported".
            capabilities: Capabilities {
                transactions: true,
                explain: true,
                ..Capabilities::default()
            },
        }
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        secret: Option<&str>,
    ) -> Result<Box<dyn Connection>> {
        // Any SSH tunnel is already established; `config` points at its local end.
        todo!()
    }
}
```

`Connection` requires `execute`, `browse`, `table_detail`, `apply_edit`, `ping`, and
`close`, with an optional `completion_scope` for autocomplete.

**Checklist for a new driver:**

1. Decode every type you can into a specific `Value` variant; route the rest to
   `Value::Unsupported` with an accurate `type_name`. Never fail a query over one column.
2. Report `ColumnSource` when the wire protocol provides it, and set
   `capabilities.column_provenance` accordingly. Without it, inline editing stays off.
3. Honour `FetchOptions::max_rows` and set `ResultSet::truncated` when you stop early.
4. Call `ResultSet::recompute_editable()` rather than setting `editable` by hand.
5. In `apply_edit`, verify exactly one row was affected and roll back otherwise.
6. Map errors to specific `Error` variants — `Auth`, `Network`, `Query { position, code }` —
   so the UI can react correctly.
7. Register the driver in `tablex_drivers::registry()` behind its Cargo feature.

## Platform support

| Platform | Minimum | Bundle | Built by CI |
|---|---|---|:---:|
| Windows | 10 1809+ | `.msi`, `.exe` (NSIS) | ✅ |
| Linux | glibc 2.35+ | `.deb`, `.rpm`, `.AppImage` | ✅ |
| macOS | 10.15+ | `.dmg` (Intel and Apple silicon) | ⬜ |
| iOS / iPadOS | 13+ | `.ipa` | ⬜ |
| Android | 8.0+ (API 26) | `.apk`, `.aab` | ⬜ |

The Linux floor is glibc 2.35 because that is what the `ubuntu-22.04` runner links
against, and a binary built against a newer glibc will not start on an older
distribution. Moving the build to an older image is the only way to lower it.

macOS is **not currently built by CI**. Distributing a `.dmg` needs an Apple Developer
Program membership to sign and notarize it, and on current macOS an unsigned download is
refused outright rather than warned about — so the artifact would be one nobody could
open. The matrix entries are kept commented in `release.yml` next to the Apple secrets
they need; restoring the platform is uncommenting them once the membership exists. The
code itself is unaffected — it compiles for macOS today, and `pnpm tauri build` on a Mac
still produces a `.dmg`.

When it returns it ships as two separate downloads rather than one universal binary:
each is half the size, and a failure on one architecture names itself instead of taking
the other down with it.

Mobile shares the entire Rust core, but it is not a build target away — see
[Why mobile is not simply another build target](#why-mobile-is-not-simply-another-build-target).

## Releases and signing

Two workflows, in `.github/workflows`:

| Workflow | Runs on | Does |
|---|---|---|
| `ci.yml` | every push and PR | typecheck, frontend tests, `cargo fmt --check`, clippy with warnings denied, full Rust test suite |
| `release.yml` | a `v*` tag, or manually | builds the Windows and Linux installers, then opens a **draft** release with them attached |

Running `release.yml` from the Actions tab builds the same installers and attaches them
to the run without touching releases — which is how to find out whether a change breaks
packaging before committing to a version number.

To cut a release:

```bash
# The tag and the version in tauri.conf.json and Cargo.toml must agree —
# nothing enforces that yet, so check it by eye.
git tag v0.2.0 && git push origin v0.2.0
```

The release is left as a draft on purpose: somebody should read the generated notes and
check the artifacts before they become something people can install.

### Signing is not set up

The installers CI produces are **unsigned**, and Windows SmartScreen warns on first run.
Acceptable for a pre-release; not acceptable for something people are asked to trust with
database credentials.

Signing needs a paid identity, bought and held by whoever publishes. Prices and
eligibility rules move — check the vendor before committing.

#### macOS — Apple Developer Program

Not needed until macOS is back in the matrix, and it is out of the matrix precisely
because this has not been bought. Kept here so the requirement is not rediscovered.

There is one route and no alternative: **$99/year**, from
[developer.apple.com/programs](https://developer.apple.com/programs/).

- **Individual** enrolment needs an Apple ID and identity verification, and usually
  completes in a day or two. The certificate is issued in your own name, which is
  visible to anyone who inspects the app.
- **Organization** enrolment shows a company name instead, but needs a legal entity and
  a [D-U-N-S number](https://developer.apple.com/support/D-U-N-S/) — free, and often the
  slowest step at one to two weeks.

Then, in Certificates, Identifiers & Profiles, create a **Developer ID Application**
certificate. Only the Account Holder can create one. Generating the signing request and
exporting the result as a `.p12` both need a Mac.

Six repository secrets, which the workflow already reads:

| Secret | Where it comes from |
|---|---|
| `APPLE_CERTIFICATE` | the `.p12`, base64-encoded |
| `APPLE_CERTIFICATE_PASSWORD` | the password set when exporting it |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAM_ID)` |
| `APPLE_ID` | the Apple ID email |
| `APPLE_PASSWORD` | an [app-specific password](https://support.apple.com/en-us/102654), not the account password |
| `APPLE_TEAM_ID` | from the membership details page |

Notarization is separate from signing and equally required — an app that is signed but
not notarized still gets refused. `tauri-action` submits it automatically once those
secrets are present.

#### Windows — a code-signing certificate

Harder than it used to be. Since the CA/Browser Forum tightened its rules in 2023, the
private key must live on certified hardware or in a signing service — a CA will no longer
issue a `.pfx` you can drop into CI. That makes **cloud signing the only practical route
for an automated build**; a USB token is unusable by a GitHub runner.

Options, roughly cheapest first:

- **[Azure Trusted Signing](https://azure.microsoft.com/products/trusted-signing)** —
  around $10/month, by far the cheapest. Individual and organization identities are both
  available, with a verification step; organizations have historically needed some
  trading history, so confirm eligibility before planning around it.
- **[Certum Open Source Code Signing](https://shop.certum.eu/open-source-code-signing.html)**
  — around €100–150/year, sold specifically for open-source projects and cheap for what
  it is. Ships as a hardware token, so it suits signing releases from a workstation more
  than from CI.
- **SSL.com eSigner**, **DigiCert KeyLocker** — cloud signing from established CAs,
  typically a few hundred dollars a year.

OV and EV differ in how SmartScreen treats them. An **EV** certificate is trusted
immediately. An **OV** one earns reputation only after enough downloads, so early users
still see the warning — which for a new project can mean months.

Wiring it in means pointing `bundle.windows` in `src-tauri/tauri.conf.json` at the
service: `signCommand` for a cloud signer, or `certificateThumbprint` for a certificate
already in the machine store. The workflow needs no change beyond adding whatever
secrets that command reads.

#### Neither is needed to build

Everything else works unsigned. The only thing signing buys is the absence of a warning —
which is worth real money precisely because that warning is the correct response to
software nobody has vouched for.

There is no auto-updater, so no update signing key is needed. Adding one later means
generating a keypair with `pnpm tauri signer generate` and holding the private half as a
repository secret.

## Contributing

Contributions are welcome. Before opening a PR:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
pnpm typecheck
```

Guidelines:

- **Tests carry their reasoning.** Test names state the invariant
  (`joins_are_not_editable`), and non-obvious cases carry a comment explaining the failure
  they prevent. A test whose purpose is unclear is worse than no test.
- **Document the *why*, not the *what*.** Comments explain tradeoffs and rejected
  alternatives. The code already says what it does.
- **No new unsafe** without a `// SAFETY:` comment justifying it.
- **Correctness beats convenience** in any conflict — see [Design decisions](#design-decisions).

There is no CLA. Contributions are accepted under the project's existing license.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work shall be dual licensed as above, without any additional terms or
conditions.

### Acknowledgements

- [TablePro](https://github.com/TableProApp/TablePro) for the design inspiration.
- [Tauri](https://tauri.app), [CodeMirror](https://codemirror.net),
  [TanStack Virtual](https://tanstack.com/virtual), and the Rust database driver ecosystem.
