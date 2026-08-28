# What DBX has that Table X does not

A read of [`t8y2/dbx`](https://github.com/t8y2/dbx) against this codebase, written down so
it can be worked through one commit at a time rather than admired as a wall.

DBX is the closest thing to a direct comparison that exists: same shape (Tauri, Rust
drivers, a webview frontend), same audience, and far wider. It claims 90+ engines in a
20 MB binary, ships a web build, a Docker image, a CLI, an MCP server and an AI assistant.
Table X is narrower on purpose — six engines, each documented down to what it cannot do —
so "DBX has it" is a reason to look, not a reason to build. Every row below carries the
judgement as well as the gap.

Ordered so that the top of the list is the part a person using Table X today would notice
first, and the bottom is the part that changes what this project *is*.

Each unchecked box is meant to be one commit. Where an item is genuinely two commits it
says so and is split.

## Where Table X is already ahead

Not for balance — for scope. These exist here and not there, and none of them should be
traded away to close a gap below: notebooks, schema designs on a canvas saved to `.erd`,
exact numerics carried as text end to end, the guarantees panel, the destructive-statement
confirmation gate, privileges and roles as a first-class panel, measured `EXPLAIN ANALYZE`
inside a rolled-back transaction, and a capability table that hides what an engine cannot
do rather than offering a button that fails.

That last one governs everything below. Anything added here is added per engine, behind
`Capabilities`, or it is not added.

---

## Tier 1 — the result grid and its exits

Small, self-contained, and the most visible per line of code spent.

- [ ] **XLSX export.** `export::Format` covers CSV, TSV, JSON, Markdown and SQL, all
      streaming. XLSX is the one format people ask a database client for that is not
      there, and it is the one format that cannot be streamed as text — the writer has to
      build a zip container. Worth doing as its own crate-level module with the same
      row-at-a-time interface so the export progress and cancellation paths do not change.
      Exact numerics go in as text, not as numbers, because a spreadsheet float is the
      rounding this project refuses everywhere else.
- [ ] **XLSX in the copy menu.** The grid already copies rows as TSV, CSV, Markdown, JSON
      and `INSERT`. Once the writer exists, the clipboard path is a second caller.
- [ ] **A find box over the fetched page.** Per-column filters exist; a plain "find text
      anywhere in these rows" does not. Match highlighting and next/previous, over the
      page in hand — deliberately not a server-side search, which is the object search in
      tier 2 and a different question.
- [ ] **Drag to resize a column.** Widths are measured from a sample of rows and are good;
      they are also final. Manual widths, persisted per table, with a double-click on the
      divider to go back to the measured one.
- [ ] **A visible WHERE / ORDER BY strip.** DBX puts the predicate and the sort above the
      grid as editable text that re-runs the query. Table X filters the page it has, which
      is honest but caps out at the fetch limit. The strip is the version that reaches the
      whole table, and it says which of the two is happening.
- [ ] **Excel import beside CSV import.** The chunked CSV reader, the column mapping and
      the preview are all reusable; only the row source changes.

## Tier 2 — schema and objects

- [ ] **Database-wide object search.** One dialog, one query per engine, matching tables,
      views, routines, columns and indexes by name across every schema — the thing a
      three-hundred-table catalog needs and a tree filter cannot be. Results open the
      object.
- [ ] **A DDL view for tables on the two engines that lack one.** PostgreSQL and SQL Server
      have no catalog function that renders a table as `CREATE TABLE`, which is why the
      tree does not offer one. The migration writer already emits exactly that statement
      from a `TableDetail` for the schema-diff and the design-to-database path. Pointing it
      at a live table closes the last two cells of that row in the README table.
- [ ] **Editing a view or routine's source.** `object_definition` reads it; nothing writes
      it back. `CREATE OR REPLACE` covers PostgreSQL, Oracle and MySQL; SQL Server has
      `ALTER VIEW` / `ALTER PROCEDURE`; SQLite has to drop and create, and should say so on
      the statement the way the trigger editor already does.
- [ ] **Executing a stored procedure with parameters.** A form built from the routine's
      declared parameters, the call statement shown before it runs, `OUT` parameters and
      result sets both surfaced. Capability-gated: SQLite has no procedures at all.
- [ ] **Column lineage for a result.** DBX ships a field-lineage dialog. The honest version
      here is narrower and more useful: for the statement in the editor, say where each
      projected column came from — which table, which expression, or "computed" when it
      cannot be traced. The provenance machinery that decides whether a result is editable
      already answers most of this; the gap is showing it per column rather than as one
      verdict.
- [ ] **PostgreSQL extensions.** List what is installed and what is available, install and
      drop. Small, and it is the panel people go to `psql` for.
- [ ] **MySQL events.** The scheduler's jobs, alongside triggers and routines, since the
      structure view already has the shape for it.

## Tier 3 — moving data around

Bigger pieces. Each one is a subsystem, not a dialog.

- [ ] **Data transfer between connections.** Copy a table — or a whole schema — from one
      live connection to another, streaming, with a type mapping per engine pair and a
      report of what could not be represented. Two commits: the core transfer with its
      mapping and its tests, then the UI over it. This is the single largest thing DBX has
      that this does not.
- [ ] **Data compare.** The schema diff compares structures; the row-level version compares
      contents of two tables on a key, and emits the `INSERT`/`UPDATE`/`DELETE` that would
      reconcile them — generated, marked destructive, and run by nobody until asked, which
      is the rule the schema diff already follows.
- [ ] **Scheduled dumps.** `export_database` exists and is streaming. What is missing is a
      timer, a retention count, and somewhere to put the file. Worth doing in the CLI first,
      where cron already exists and needs no window — the desktop scheduler is the second
      commit and the weaker half.
- [ ] **Opening a file without a connection.** Drag a CSV, JSON or Parquet file onto the
      window and get a grid. The first two need no new dependency. Parquet does, and the
      question of whether to take DuckDB for it belongs with the DuckDB driver in tier 4
      rather than here.
- [ ] **Importing connections from DBeaver and Navicat.** Both keep their profiles in a
      documented file. A parser each, mapping onto `ConnectionConfig`, with anything that
      does not map reported rather than dropped — silently losing an SSH hop is worse than
      refusing the import.
- [ ] **Encrypted export and import of settings.** Connections, snippets, appearance and
      history in one file, passphrase-encrypted, so a new machine is one import. The
      keychain entries are the hard part: secrets have to be re-derived on the way in, not
      copied.

## Tier 4 — engines that are not SQL

The README already says why these are structural rather than "one more `Driver` impl":
the editor, autocomplete, formatter, `EXPLAIN`, diff and privileges all assume a SQL
statement. Doing these honestly means the workspace learning to host a second query mode.
That is the first commit, and it is a prerequisite for the rest of this tier.

- [ ] **A non-SQL query mode in the workspace.** A tab kind whose editor, runner and result
      shape come from the driver rather than from the SQL assumptions. No engine attached —
      just the seam, with the SQL path moved onto it unchanged.
- [ ] **MongoDB.** `tablex_core::documents` is already written and tested: heterogeneous
      documents into columns and rows, absent fields kept distinct from null ones, extended
      JSON decoded so `$numberDecimal` stays exact. The driver is pure Rust. Two commits:
      driver and catalog, then the document browser with CRUD and pagination.
- [ ] **Redis.** Keyspace browsing by pattern, TTL editing, the six value types each with
      its own view, and a command runner. Nothing about it fits a result grid, which is why
      it comes after the mode above.
- [ ] **DuckDB.** Embedded, like SQLite, and the natural home for the Parquet half of the
      file preview above. It also brings the compile-time cost DBX warns about in its own
      README, so it is a Cargo feature.
- [ ] **Elasticsearch.** Only if the non-SQL mode has already proven itself on two engines.
      Listed for completeness, not as a commitment.

## Tier 5 — platform and distribution

- [ ] **Install by package manager.** DBX is on Homebrew, Scoop, WinGet and Flatpak.
      Table X builds MSI, NSIS, `.deb`, `.rpm` and AppImage on every tag and stops there.
      Scoop and WinGet want only a manifest and a checksum, so they are cheap and come
      first; Homebrew and Flatpak want signing, which is the open row on milestone 1 and
      blocks them for the same reason macOS is not built.
- [ ] **Applying an update, not just noticing one.** The update check exists and sets a
      badge. Downloading and installing it is the other half, and it needs signing to be
      safe — an unsigned auto-updater is a worse idea than no auto-updater.
- [ ] **Translations.** DBX ships English, 简体中文 and Español. Every string here is a
      literal in a component. The first commit is extraction and a lookup, with English as
      the only catalogue; a second language is a separate commit and a separate skill.
- [ ] **Test data generation.** Fill a table with plausible rows from per-column generators.
      Genuinely useful against a schema design that has never held data, which is a path
      this project already has and DBX does not.
- [ ] **Sharing a query as an image.** DBX calls it a code snapshot. Cheap, and it is how
      people paste SQL into an issue.
- [ ] **AI assistance.** Named in the README as out of scope for milestone 1 and tracked
      for later; this is the row that keeps it tracked. If it is built, it is built as a
      panel that writes into the editor and never runs anything itself, and the existing
      destructive-statement gate stands in front of anything it produces.
- [ ] **A web build and a Docker image.** DBX self-hosts for team access over the same
      backend. `tablex-core` and the drivers are already free of Tauri, which is most of
      what makes this possible — but the authentication, the multi-user session registry
      and the fact that the keychain has no meaning on a server are all real work, and it
      is a different product decision rather than a feature.

## Not doing

Written down so the question stops being asked.

- **Message queue administration** (Kafka, Pulsar, RocketMQ), **etcd, ZooKeeper, Nacos,
  Consul KV**. DBX has grown into an infrastructure console. This is a database client.
- **A JDBC bridge.** It is how DBX reaches most of its 90+ engines, and it would mean
  shipping or requiring a JVM — which is precisely the dependency the 20 MB claim and this
  project's design goal 3 both exist to avoid.
- **A third-party plugin system.** Already listed as out of scope for milestone 1. A plugin
  API is a compatibility promise, and it is too early to make one.
- **Counting engines.** Five of the ninety are supported by a generic JDBC profile and a
  dialect file. The table in the README says what each engine actually does, cell by cell,
  and that is the number worth competing on.
