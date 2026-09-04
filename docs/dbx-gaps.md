# Table X against DBX

A read of [`t8y2/dbx`](https://github.com/t8y2/dbx) at `ce36a7f` (5 September 2026)
against this codebase at `3f238cd` (4 September 2026). It replaces the earlier version of
this file, which was a week old and had not been worked through; DBX has moved since and
some of the judgements had to be re-made.

Sources: both READMEs, the forty English pages of the DBX docs site, DBX's component tree
and settings store, and this repository's components, command registry, IPC surface and
`Capabilities` struct. Two screenshots from each side were looked at for the UX notes.

DBX is the closest comparison that exists: same shape (Tauri, Rust core, webview
frontend), same audience, and far wider. Table X is narrower on purpose, so "DBX has it" is
a reason to look, not a reason to build. Every gap below carries the judgement as well as
the gap. Where something is listed under "not doing", it is listed so the question stops
being asked.

## The two projects in numbers

| | Table X | DBX |
|---|---|---|
| Engines | 6 (PostgreSQL, MySQL/MariaDB, SQLite, SQL Server, ClickHouse, Oracle), each documented cell by cell | 90+ profiles; 82 connection-type plugins; many reached through a JDBC or Go agent |
| Frontend | React 19, Tailwind 4, zustand | Vue 3, shadcn-vue, Tailwind |
| Frontend size | ~21,000 lines under `src/` | 44 component areas; ~990 spec files |
| Rust tests | 585 | ~6,900 |
| Interface languages | English only, strings inline | 9 locales (en, es, it, ja, ko, pt-BR, tr, zh-CN, zh-TW) |
| Deployment | Desktop (Windows, Linux built; macOS not signed) | Desktop, Docker/Web, CLI, MCP server, npm packages |
| Install channels | MSI, NSIS, deb, rpm, AppImage from tags; unsigned | Homebrew, Scoop, WinGet, Flatpak; code-signed via a sponsor |
| Documentation | README | Docs site, 40 pages × 3 languages, in-app changelog |
| Secrets at rest | OS keychain | DBX's own SQLite file, secrets kept apart from connection JSON |

## Where Table X is ahead

Not for balance, for scope. These exist here and not there, and none of them should be
traded away to close a gap below.

- **Exact numerics, made visible.** `NUMERIC`/`DECIMAL` travel as text end to end, the
  grid marks exact columns, and the guarantees panel says what is promised about a result.
  DBX's data grid docs say nothing about precision at all.
- **Capabilities default to unsupported, and the README matrix says so per cell.** DBX
  documents the same idea ("menu availability is more meaningful than a static list") but
  its docs are full of "depends on driver, version and privileges". Ours can be checked
  against a table.
- **Inline edits are parameterised, keyed on the original row values, and rolled back
  unless exactly one row changed.** DBX stages edits and shows the SQL, which is a better
  review experience (see grid gaps) but a weaker guarantee.
- **Measured `EXPLAIN ANALYZE` inside a rolled-back transaction**, offered only on the one
  engine that can take it back. DBX's explain is estimates only.
- **Notebooks**: prose and queries in one saved document. DBX has nothing like it.
- **Schema designs on a canvas, saved to `.erd`**, generated from a live schema, synced back
  as a reviewed script. DBX's ER diagram is read-only.
- **Privileges and roles as a first-class panel**, with role inheritance and SQL Server
  denials, names kept in the engine's own words. DBX has user administration on some
  engines but no cross-engine grants view.
- **Schema diff that runs nothing.** DBX can deploy the diff to the target, in a
  transaction, with rollback SQL. That is more capable; ours is safer. Listed on both sides.
- **The destructive-statement gate keyed on the colour tag**, with the connection name
  typed for unbounded statements. DBX's production protection is broader (below) but its
  confirmation is a dialog, not a typed name.
- **SSH without trust-on-first-use**, agent authentication with no key in process,
  multi-hop `ProxyJump`. DBX prompts to accept an unknown host key and offers key-then-
  password fallback, which is friendlier and weaker.
- **Query history that drops credential-assigning statements** rather than redacting them.
- **The `ORDER BY` warning** on a paged result, with a one-click fix that appends the key
  columns. DBX makes you type the `ORDER BY`.
- **Charts with no charting dependency**, values read back from text.
- **Row estimates in the tree**, next to every table, and the "1 connected" header. Small,
  and the DBX tree does not have it.
- **Privacy of the update check.** No version, platform or identifier sent. DBX's
  auto-updater is a full Tauri updater.
- **The command palette is over actions.** DBX's `Mod+P` is over objects and saved SQL;
  it has no action palette.
- **Size and legibility of the codebase.** One person can hold it. DBX's own handoff
  document lists forty active files for one feature.

## What DBX does better, area by area

Ordered so the top is what a person using Table X today would notice first. Each unchecked
box is meant to be one commit; where it is two, it says so.

### 1. The result grid

DBX's grid is the single largest gap. Ours renders, filters the fetched page, edits in place
and pages; theirs is a workbench.

- [ ] **Drag to resize a column, double-click the divider to fit.** Widths here are
      measured from a sample of rows and are final.
- [ ] **Drag headers to reorder, and persist widths, order and frozen state per table or
      query.** One commit for resize and reorder, one for persistence.
- [ ] **Freeze columns left or right.**
- [ ] **Show, hide and search columns** from a column picker.
- [ ] **A transposed view** (DBX binds it to `Tab`): one record as a column of label/value
      pairs. Our row-details panel is the nearest thing and it is a side panel, not a mode.
- [ ] **Find over the loaded page**, with match highlighting and next/previous.
      Per-column operators exist; "find this text anywhere" does not.
- [ ] **A filter builder** with list and range filters, `LIKE`/`NOT LIKE` from the context
      menu, and a text-filter workbench. Our quick-filter menu covers equals, not, contains,
      null.
- [ ] **A visible `WHERE` / `ORDER BY` strip above table rows**, with metadata completion,
      that re-runs the query server-side, and a default-sort setting for opened tables.
      Ours filters the page it has, which is honest but caps at the fetch limit. The strip
      should say which of the two is happening.
- [ ] **Selection summary**: count and sum of the selected cells in the status bar.
- [ ] **Rectangular range selection, and paste TSV from the clipboard as new rows.**
- [ ] **Staged edits with a SQL preview before save**, undo/redo in the preview, and a
      commit/rollback pair. Ours applies each cell edit the moment it is confirmed. Keep
      the per-row exactness, but show the `UPDATE` that will run, and consider a staged
      mode where several edits go as one reviewed batch.
- [ ] **Bulk edit of a selection**, and DBX's MySQL-only "update by `WHERE`" for rows
      outside the loaded page, with an estimated affected-row count first.
- [ ] **Set default / generate value for a cell.** Our set-value menu has NULL and empty
      text; the stamp-with-now commit was the first typed one.
- [ ] **Typed editors for enums and temporal values.** JSON, boolean and hex exist here;
      a date/time picker and an enum list do not.
- [ ] **Cell detail extras**: image preview for URLs and binary, geometry on a map with
      SRID detection, JSON path inspection, and a diff of the original value against the
      edited one. The last is cheap and belongs with undo.
- [ ] **Column formatters**: display-only date formats, masking, JSON-path extraction,
      templates. Never mutate data.
- [ ] **Foreign-key navigation from a cell** to the referenced row. The provenance
      machinery already knows the target table.
- [ ] **Copy as `UPDATE`, copy column names with a quoting choice, and a copy-format
      configuration** (null text, quote policy, one multi-row `INSERT` or one per row,
      exclude primary keys, skip generated columns). Our copy menu has TSV, CSV, Markdown,
      JSON and `INSERT`.
- [ ] **XLSX export**, with a new worksheet when one fills. `export::Format` covers CSV,
      TSV, JSON, Markdown and SQL, all streaming; XLSX cannot be streamed as text and needs
      a zip writer. Exact numerics go in as text, not as spreadsheet floats. One commit for
      the writer, one for the clipboard path.
- [ ] **Export scope**: current selection versus whole result, said out loud in the dialog.
- [ ] **Multiple result tabs**: execute in a new result (`Mod+\`), pin a result, keep run
      history, switch between tabbed and list layouts.
- [ ] **Auto refresh** for small, changing result sets, refused while there are unsaved
      edits.
- [ ] **Grid snapshot as an image**, with theme, row numbers and field metadata options.
- [ ] **A type colour scheme** for cell values by database type.
- [ ] **The executed SQL in the footer.** DBX shows the statement it ran under table rows;
      our footer shows the timing.

### 2. The SQL editor

- [ ] **Preview DML changes before executing.** DBX rewrites an `UPDATE`, `INSERT` or
      `DELETE` into an equivalent `SELECT` and opens it in a result tab, with one
      `column (new)` per assignment. This is the best idea in the DBX editor and it fits
      our correctness story exactly. Falls back to "preview not supported" for shapes it
      cannot rewrite.
- [ ] **Parameters.** `?`, `:name`, `${name}`, `#{name}`, `@name` and `@set name = 42;`
      open a prompt before execution, typed as string, number, boolean, NULL or raw SQL,
      with per-engine toggles for forms that clash with native syntax.
- [ ] **An execution-target picker** for multi-statement text: which statement the caret
      is in, marked in the gutter, with a preview. Ours runs the selection or everything.
- [ ] **Semantic diagnostics**: unresolved tables, columns and aliases underlined before
      running. We underline the server's error position after.
- [ ] **`JOIN` completion from foreign keys, and alias / CTE / subquery scope.** Worth
      checking how far `completion_scope` already reaches; DBX documents all three.
- [ ] **`Ctrl+Click` a table name** to open its rows or locate it in the tree; drag a
      table or column from the tree into the editor.
- [ ] **Code folding**, case conversion and a naming-style toggle for the selection.
      `defaultKeymap` already gives comment toggling, line move and line delete.
- [ ] **Compact SQL** (the inverse of format), and **formatter options**: keyword and
      identifier case, indent style, tab width, operator newline, expression width, with a
      JSON import/export. Our formatter has no options.
- [ ] **Snippets that expand from a prefix in completion, with tab-stop placeholders**
      (`sel`, `ins`, `cte`, `${1:columns}`). Our saved queries are named and reached from
      the palette; they do not expand.
- [ ] **SQL quick actions**: a template with `${table}` bound to a shortcut, run against
      the selected identifier.
- [ ] **Open and save `.sql` files on disk, a SQL file tree panel, and detection of
      external changes.** Our only file path is "import SQL file", which executes it.
- [ ] **A SQL library with folders**, `Mod+S` to save, searchable from quick open.
- [ ] **Editor themes separate from the app theme**, and a custom theme editor with live
      preview and JSON import/export.
- [ ] **A column info panel** beside the editor for the table under the caret.
- [ ] **Code snapshot**: the query as an image, with a title and window chrome. Cheap, and
      it is how people paste SQL into an issue.
- [ ] **Query history filtered by date range**, and a distinction between hand-written and
      AI-produced statements. Ours is searchable across connections and exportable, which
      DBX's is not.

### 3. Tabs, windows and layout

- [ ] **Split editor groups**, and **detach a tab into its own window** with a "return to
      main window" control.
- [ ] **Tab reorder by drag, pinned tabs, a tab switcher dialog, `Mod+1`–`9`, previous /
      next tab, and back / forward through tab visit history.** We close on middle-click
      and stop there.
- [ ] **A tab-restore policy**: all, pinned only, or none. We restore everything.
- [ ] **Configurable keyboard shortcuts** with scopes (editor, grid, sidebar, global),
      conflict detection, and a settings page that shows the effective binding. Ours are
      fixed and listed in tooltips.
- [ ] **`Mod+B` to toggle the sidebar and `Mod+=` / `Mod+-` / `Mod+0` for UI zoom.** The
      sidebar button exists; the shortcut and zoom do not.
- [ ] **A welcome screen** with quick connections, recent SQL, a shortcut cheat-sheet and
      a tip. Ours is one sentence in an empty pane.
- [ ] **A background-task centre** for exports, transfers and backups, with a failed-count
      badge. Our export progress is per export.
- [ ] **A toolbar**: new connection, new query, data transfer, driver manager, theme
      toggle, updates. Our top bar is the app name, a connected count and a gear.
- [ ] **Tray icon, quit-on-close prompt, native title-bar theme sync.**
- [ ] **Applying an update, not just noticing one.** Needs signing to be safe; an unsigned
      auto-updater is worse than none.
- [ ] **Translations.** First commit is extraction and a lookup with English as the only
      catalogue.

### 4. Sidebar and schema browser

- [ ] **Connection groups that nest, drag to group, multi-select with batch actions
      (group, copy, disconnect), pin, notes, duplicate, and a tooltip with host and port.**
      We have folders and a colour tag.
- [ ] **Connection search with a regex toggle and an active-connections filter.**
- [ ] **Visible databases / schemas dialog**: hide system schemas and prefix-matched
      objects, auto-add newly created ones.
- [ ] **Comments, type summaries and object counts in the tree**, each switchable to keep
      large schemas quiet.
- [ ] **Expand all / collapse all, and "locate the open tab's object in the tree".**
- [ ] **Server-side tree search with debouncing**, and a local index for big catalogs.
- [ ] **Quick open (`Mod+P`)** over connections, databases, schemas, tables, views and
      saved SQL. Our palette could grow this as a second mode.
- [ ] **Database-wide object search**: one dialog, one query per engine, matching tables,
      views, routines, columns and indexes by name across schemas. A tree filter cannot be
      this.
- [ ] **An object browser**: list or grid of every object in a schema with row counts,
      sizes and timestamps, multi-select, bulk drop / truncate / export.
- [ ] **Richer context menus**: rename table, create / rename / drop database and schema
      with charset and collation, copy a table and paste it into another database,
      generate `SELECT` / `INSERT` / `UPDATE` / `DELETE` templates, reveal a file database
      in the file manager. Ours has open rows, new SELECT tab, CREATE statement, edit
      script, copy name, import CSV, refresh, truncate.
- [ ] **View and edit the source of views, routines and triggers**, with search inside
      the source. `object_definition` reads it; nothing writes it back.
- [ ] **Execute a stored procedure** from a form built from its parameters, showing the
      call before it runs, with `OUT` parameters and result sets both surfaced.
- [ ] **A `CREATE` statement for tables on PostgreSQL and SQL Server.** The migration
      writer already emits one from a `TableDetail`; pointing it at a live table closes the
      last two cells in the README matrix.
- [ ] **PostgreSQL extensions** (list, install, drop), **MySQL events**, and a custom-types
      panel.
- [ ] **Field lineage**: where a column comes from and what a change would affect, from
      foreign keys, view source and query history, labelled certain / likely / possible.
      The honest version here is narrower: per projected column of the current statement,
      which table or expression it came from.
- [ ] **Database documentation**: the schema rendered as a browsable reference with
      Markdown notes per table and column, colour-coded groups, and DBML export for
      dbdiagram.io and a CI drift check. Notes live in a JSON file next to migrations.
      Pairs naturally with our schema designs.

### 5. Connections and safety

- [ ] **Paste a connection URL** and have the form filled. Every driver here has a URL
      form; `url.rs` in the core already parses some.
- [ ] **Production protection per connection or per database**, with a watermark in the
      workbench and a fresh confirmation for every detected write. Our gate is per
      connection via the colour tag; DBX's scope is finer and its badge is always visible.
- [ ] **A temporary write unlock (1 or 5 minutes) on a read-only connection**, with the
      remaining time shown and a "lock now". Our read-only flag is a hard wall.
- [ ] **A dangerous-SQL confirmation as a setting** independent of the production gate.
      Ours is on the connection ("Confirm destructive statements").
- [ ] **Ask for the password at connect time** instead of storing it.
- [ ] **A categorised connection-test failure**: auth, timeout, TLS, unreachable, with a
      hint each.
- [ ] **More transports**: SOCKS5 and HTTP CONNECT proxies, shared tunnel profiles used by
      several connections, key-then-password fallback, a connect timeout, and a remote
      SQLite file opened over SSH through a worker on the far host. The PHP HTTP tunnel is
      not worth copying.
- [ ] **Import connections from Navicat, DBeaver and DataGrip**, preserving groups. A
      parser each; anything that does not map is reported, not dropped.
- [ ] **Encrypted export and import of connections** (AES-256-GCM, PBKDF2), selecting
      which to include, deduplicated on import. Keychain entries have to be re-derived on
      the way in.
- [ ] **Cloud sync of settings and saved SQL** via WebDAV, GitHub Gist or Gitee, Argon2id
      + AES-256-GCM, secrets excluded unless opted in. The WebDAV half is the useful one.

### 6. Moving data

Bigger pieces. Each one is a subsystem.

- [ ] **Table import from TSV, delimited text, JSON, Excel and literal-`INSERT` SQL**,
      with encoding detection, a title-row and range picker, "empty string as NULL",
      truncate-then-import, and a **create-new-table mode** that infers types and accepts
      several files at once. Our CSV import appends to an existing table. Excel beside CSV
      is the first commit; create-table the second.
- [ ] **Data transfer between connections**: copy tables or a schema from one live
      connection to another, streaming, with type mapping, append / overwrite / upsert,
      per-table failure reporting, and copy/paste of a table in the tree as the entry
      point. Two commits: core with tests, then UI. Still the largest thing DBX has that
      we do not.
- [ ] **Data compare**: rows of two tables on a key, emitting the `INSERT` / `UPDATE` /
      `DELETE` that would reconcile them, generated and marked, run by nobody until asked.
- [ ] **Database export options**: views, sequences, routines; `DROP TABLE IF EXISTS`;
      export every database on a server to a directory; MySQL `CREATE DATABASE` preamble;
      omit `AUTO_INCREMENT`; reveal the file afterwards.
- [ ] **Scheduled backups** with retention and history, plus a one-shot "back up now".
      Do it in the CLI first, where cron exists; the desktop scheduler is the weaker half
      and DBX admits it only runs while the app is open.
- [ ] **Multi-file SQL execution** with per-file statistics, continue-on-error, `GO`
      batches, dollar quoting and MySQL executable comments. Ours honours `DELIMITER` and
      shows progress as of this week.
- [ ] **Open a file without a connection**: drop a CSV, JSON or Parquet file and get a
      grid. Parquet needs DuckDB, which is a tier-7 question.
- [ ] **Test-data generation**: per-column generators, batch size, insertion in
      foreign-key order, saved profiles. Especially useful against a schema design that
      has never held data, which we have and DBX does not.

### 7. Engines that are not SQL

The README already says why these are structural: the editor, autocomplete, formatter,
`EXPLAIN`, diff and privileges all assume a SQL statement. The first commit is the seam.

- [ ] **A non-SQL query mode in the workspace**, with the SQL path moved onto it unchanged.
- [ ] **MongoDB.** `tablex_core::documents` is written and tested. Driver and catalog,
      then a document browser with CRUD, pagination, index management and GridFS.
- [ ] **Redis.** Keyspace by pattern, TTL editing, the six value types, a command runner,
      a danger classifier for commands.
- [ ] **DuckDB**, as a Cargo feature, and the home for Parquet preview.
- [ ] **Elasticsearch**, only if the mode has proven itself on two engines.

### 8. AI, agents and automation

- [ ] **An AI panel.** DBX's has ten provider presets, local CLI agents (Claude Code,
      Codex), `@table` mentions, prompt templates with per-engine defaults, an Ask mode that
      never executes and an Agent mode whose write authorisation is bound to the exact SQL,
      connection and database for one run and never granted in production. If built here,
      it writes into the editor, never runs anything itself, and the destructive gate
      stands in front of whatever it produces.
- [ ] **MCP depth.** Ours: query, list, describe, explain, read-only by default, row caps,
      every call audited. DBX: 18 tools, a central policy with an allowlist and three modes
      (read only, data read/write, full access), stateful sessions, batch execution, and
      "open this table in the app". The policy UI and the per-connection scope are the
      parts worth having.
- [ ] **CLI additions**: `doctor`, `capabilities`, `context` (compact schema for a model),
      `open` (a table in the running app), `dbml`. Ours has `diff --exit-code` and `import`,
      which DBX's does not.
- [ ] **A web build and a Docker image.** The core is already free of Tauri. The
      authentication, the multi-user session registry and the keychain having no meaning
      on a server are the work, and it is a product decision.

### 9. Distribution and documentation

- [ ] **Scoop and WinGet** want a manifest and a checksum; cheap. Homebrew and Flatpak
      want signing.
- [ ] **A docs site.** Forty pages in three languages is DBX's biggest non-code asset. A
      README cannot carry keyboard shortcuts, safety model, and per-feature boundaries as
      the app grows. Start with the README split into pages.
- [ ] **An in-app changelog** panel.

## UX notes, from the screenshots

Things a person feels before they can name them.

- **DBX is toolbar-driven and dense**: a top bar of labelled actions, a right-hand panel
  that holds the AI chat or the SQL library, a status bar that shows the executed
  statement, total rows, timing and paging. Table X is text-labelled and quiet: one row of
  buttons above the grid, a footer with timing. Ours reads more easily; theirs shows more
  state. The middle ground is a status bar that says what ran and how many rows there are.
- **DBX puts the query context in the tab title** (`table@database.schema`) and a chip row
  under it (`sanguo_characters` · `public@test_geometry` · 7 fields). We put the database
  under the table name in the tab and rely on the tree for the rest.
- **DBX's grid context menu is a workbench**: sort, filter, cell / column / row details,
  copy, bulk edit, transpose, select region, clone N rows, delete N rows, export. Ours is
  copy, set value, duplicate, quick filter. The counts in "clone 17 rows" are a nice touch.
- **DBX marks the active database with a pin and the connection with a green dot**; ours
  marks the connection with a coloured dot and the open object in the tree. Both fine.
- **DBX's headers carry sort arrows and a per-column menu**; ours carry the type and a
  filter cell. The filter cell is more direct; the menu scales to more actions.
- **DBX has a visible "Canvas" button on table rows** and per-table `WHERE` / `ORDER BY`
  inputs; ours has "Filter rows…", which filters the page.
- **Where ours is friendlier**: tooltips that explain rather than label ("Without an ORDER
  BY the server may return rows in a different order each time…"), the "Double-click to
  edit" hint, the `RO` badge on a read-only connection, the row estimates beside every
  table, the appearance dialog that applies as you change it and describes each theme in a
  line, and the page-size control in the footer.
- **Where ours is emptier**: the welcome pane, the top bar, the absence of any tab or
  sidebar shortcuts, and no way to see the shortcut list without hovering things.

## What DBX pays for its breadth

Worth knowing before copying anything.

- Most of the 90+ engines are reached through a JVM agent or a vendor JDBC JAR the user
  has to obtain, so "20 MB" is the download, not the footprint.
- The docs say "depends on driver, version and privileges" on nearly every page; the
  capability matrix was deliberately dropped from the docs as "brittle". Ours is the table.
- Secrets live in DBX's own SQLite file, not the OS keychain.
- Scheduled backups run only while the desktop app is open. Cloud sync does not merge.
- Desktop and Web are "not feature-identical", and the list of Desktop-only things is
  long: SQL files, local CLI agents, backups, deep links.
- The README leads with six sponsor cards and a QQ group before the feature list.

## Not doing

- **Message queue administration, etcd, ZooKeeper, Nacos, Consul, HBase, vector stores.**
  DBX has grown into an infrastructure console. This is a database client.
- **A JDBC bridge.** It would mean shipping or requiring a JVM, which is the dependency
  design goal 3 exists to avoid.
- **A third-party plugin system and a driver store.** A plugin API is a compatibility
  promise, and it is too early to make one.
- **The PHP HTTP tunnel.** A relay script on a web server is a liability, not a feature.
- **Counting engines.** The README matrix says what each engine does, cell by cell.

## Suggested order

If the boxes above are worked top to bottom the first month goes entirely into the grid,
which is right. A shorter list of the ten that change the daily experience most, in order:

1. Column resize, reorder, hide, freeze, persisted per table.
2. Find over the loaded page.
3. The `WHERE` / `ORDER BY` strip.
4. Execute in a new result tab, and keep results.
5. Preview DML as a `SELECT` before running it.
6. Parameters prompt.
7. XLSX export.
8. Quick open over objects, tab reorder and pin, configurable shortcuts.
9. Excel import and create-table-on-import.
10. Production protection per database, with the always-visible badge.
