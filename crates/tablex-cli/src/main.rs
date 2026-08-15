//! `tablex` — the same drivers, without the window.
//!
//! The core crate was built with no GUI dependency from the start, which makes
//! this a thin front end rather than a second implementation: the drivers, the
//! exact-numeric handling, the streaming export writers and the schema diff are
//! all the ones the desktop application uses. A row exported here is the same
//! bytes as a row exported there, because it is the same writer.
//!
//! # Credentials
//!
//! There is no keychain in CI, so a connection is given as a URL. A password
//! may be in that URL, but `--url` on a command line is visible to every other
//! process on the machine through `ps`, so `TABLEX_URL` and `TABLEX_PASSWORD`
//! are read as environment variables and are the form to prefer in a pipeline.
//!
//! # Streams
//!
//! Data goes to stdout and everything else to stderr, so `tablex query … > out.csv`
//! produces a file containing only rows.

mod mcp;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::io::Write;
use tablex_core::{
    driver::{Connection, Driver, FetchOptions},
    export::Format,
    result::StatementResult,
};

#[derive(Parser)]
#[command(
    name = "tablex",
    version,
    about = "Query, export, import and compare databases from a script.",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Suppress progress and summaries on stderr. Errors are still reported.
    #[arg(long, short, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Run a statement and print the rows.
    Query {
        #[command(flatten)]
        target: Target,
        /// The statement to run.
        sql: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Write to a file instead of stdout.
        #[arg(long, short)]
        output: Option<String>,
        /// Stop after this many rows. Omit for no limit.
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Stream a table or a query to a file.
    Export {
        #[command(flatten)]
        target: Target,
        /// Table to export. Give this or --query.
        #[arg(long, conflicts_with = "query")]
        table: Option<String>,
        /// Statement to export the result of.
        #[arg(long)]
        query: Option<String>,
        #[arg(long, value_enum, default_value_t = FileFormat::Csv)]
        format: FileFormat,
        /// Write to a file instead of stdout.
        #[arg(long, short)]
        output: Option<String>,
    },

    /// Load a CSV or SQL file into a database.
    Import {
        #[command(flatten)]
        target: Target,
        /// File to read.
        #[arg(long, short)]
        file: String,
        /// Table to load a CSV into. Not used for SQL files.
        #[arg(long)]
        table: Option<String>,
        /// Field separator. Sniffed from the file when omitted.
        #[arg(long)]
        delimiter: Option<char>,
        /// The file's first row is data, not column names.
        #[arg(long)]
        no_header: bool,
    },

    /// Compare two schemas and print the statements that reconcile them.
    Diff {
        /// The side the script would be run against.
        #[arg(long, env = "TABLEX_URL")]
        from: String,
        /// The side to make it look like.
        #[arg(long)]
        to: String,
        /// Schema on both sides. Defaults to the driver's usual one.
        #[arg(long)]
        schema: Option<String>,
        /// Exit 1 when there are differences, for a CI drift check.
        #[arg(long)]
        exit_code: bool,
    },

    /// Serve the Model Context Protocol over stdin and stdout.
    ///
    /// Read-only unless --allow-writes is given: an agent handed a connection
    /// string did not choose anything, and the two mistakes do not cost the
    /// same.
    Mcp {
        #[command(flatten)]
        target: Target,
        /// Execute statements that look like writes.
        #[arg(long)]
        allow_writes: bool,
        /// Largest number of rows any one call may return.
        #[arg(long, default_value_t = 1000)]
        max_rows: usize,
        /// Append every call to this file as JSON lines.
        #[arg(long)]
        audit: Option<String>,
    },

    /// List the tables in a schema.
    Tables {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        schema: Option<String>,
    },
}

/// Where to connect, shared by every subcommand that connects to one database.
#[derive(clap::Args)]
struct Target {
    /// Connection URL, e.g. postgres://user@host/db or sqlite:///data/app.db.
    ///
    /// Prefer the environment variable: an argument is visible to every process
    /// on the machine.
    #[arg(long, env = "TABLEX_URL")]
    url: String,

    /// Password, if it is not in the URL.
    #[arg(long, env = "TABLEX_PASSWORD", hide_env_values = true)]
    password: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Aligned columns for reading.
    Table,
    Csv,
    Json,
    /// One JSON object per line, for piping into jq or a log shipper.
    Ndjson,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum FileFormat {
    Csv,
    Json,
    /// INSERT statements.
    Sql,
}

impl From<FileFormat> for Format {
    fn from(value: FileFormat) -> Self {
        match value {
            FileFormat::Csv => Format::Csv,
            FileFormat::Json => Format::Json,
            FileFormat::Sql => Format::Sql,
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            // The chain, not just the head: "could not connect" without the
            // reason underneath is the least useful half of the message.
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<std::process::ExitCode> {
    let note = |message: &str| {
        if !cli.quiet {
            eprintln!("{message}");
        }
    };

    match cli.command {
        Command::Query {
            target,
            sql,
            format,
            output,
            limit,
        } => {
            let mut conn = connect(&target).await?;
            let outcome = conn
                .execute(
                    &sql,
                    &FetchOptions {
                        max_rows: limit,
                        offset: 0,
                        timeout_secs: None,
                    },
                )
                .await
                .context("the statement failed")?;

            let mut out = open_output(output.as_deref())?;
            for statement in &outcome.statements {
                match statement {
                    StatementResult::Rows(rows) => write_rows(&mut out, rows, format)?,
                    StatementResult::Affected { rows_affected, .. } => {
                        note(&format!("{rows_affected} rows affected"));
                    }
                }
            }
            out.flush()?;
            note(&format!("{} ms", outcome.elapsed_ms));
            let _ = conn.close().await;
            Ok(std::process::ExitCode::SUCCESS)
        }

        Command::Export {
            target,
            table,
            query,
            format,
            output,
        } => {
            let mut conn = connect(&target).await?;
            let quote = driver_for(&target)?.info().capabilities.identifier_quote;

            let (sql, label) = match (&table, &query) {
                (Some(table), _) => (
                    format!(
                        "SELECT * FROM {}",
                        tablex_core::sql::quote_ident(table, quote)
                    ),
                    table.clone(),
                ),
                (None, Some(query)) => (query.clone(), "result".to_string()),
                (None, None) => return Err(anyhow!("give --table or --query")),
            };

            let sink = open_output(output.as_deref())?;
            // Returning Ok keeps the stream going; this is where a cancel or a
            // progress report would hook in, and the CLI needs neither.
            let on_batch = |_rows: u64| Ok(());
            let mut stream =
                tablex_core::export::StreamSink::new(sink, format.into(), label, quote, &on_batch);

            let opts = FetchOptions {
                max_rows: None,
                offset: 0,
                timeout_secs: None,
            };
            let result = conn.stream(&sql, &opts, &mut stream).await;
            let written = stream.rows();

            match result {
                Ok(_) => {
                    stream.finish().context("could not finish writing")?;
                    note(&format!("{written} rows"));
                    let _ = conn.close().await;
                    Ok(std::process::ExitCode::SUCCESS)
                }
                Err(e) => {
                    // The half-written file is left for the caller to see rather
                    // than deleted: unlike the desktop app, this may be writing
                    // to stdout, and there is nothing to delete there.
                    let _ = conn.close().await;
                    Err(anyhow::Error::new(e).context("the export stopped partway"))
                }
            }
        }

        Command::Import {
            target,
            file,
            table,
            delimiter,
            no_header,
        } => {
            let mut conn = connect(&target).await?;
            let quote = driver_for(&target)?.info().capabilities.identifier_quote;

            let applied = if file.to_lowercase().ends_with(".sql") {
                import_sql(&mut *conn, &file).await?
            } else {
                let table = table
                    .ok_or_else(|| anyhow!("--table is required when importing a delimited file"))?;
                import_csv(&mut *conn, &file, &table, delimiter, !no_header, quote).await?
            };

            note(&format!("{applied} applied"));
            let _ = conn.close().await;
            Ok(std::process::ExitCode::SUCCESS)
        }

        Command::Diff {
            from,
            to,
            schema,
            exit_code,
        } => {
            let mut left = connect_url(&from, None).await?;
            let mut right = connect_url(&to, None).await?;

            note("reading both schemas…");
            let before = snapshot(&mut *left, schema.as_deref(), "from").await?;
            let after = snapshot(&mut *right, schema.as_deref(), "to").await?;

            let driver = tablex_core::url::parse(&from)?.config.driver;
            let changes = tablex_core::diff::diff(&before, &after);
            let script = tablex_core::diff::migration(
                &changes,
                tablex_core::diff::Dialect::for_driver(&driver),
            );

            for statement in &script {
                if let Some(note) = &statement.note {
                    println!("-- {note}");
                }
                println!("{}\n", statement.sql);
            }

            note(&format!(
                "{} changes, {} destructive",
                changes.len(),
                script.iter().filter(|s| s.destructive).count()
            ));

            let _ = left.close().await;
            let _ = right.close().await;

            // A drift check wants a non-zero exit when the schemas disagree,
            // and every other use wants success. Opt in rather than guess.
            Ok(if exit_code && !changes.is_empty() {
                std::process::ExitCode::FAILURE
            } else {
                std::process::ExitCode::SUCCESS
            })
        }

        Command::Mcp {
            target,
            allow_writes,
            max_rows,
            audit,
        } => {
            let mut conn = connect(&target).await?;
            note(if allow_writes {
                "tablex mcp: WRITES ENABLED"
            } else {
                "tablex mcp: read-only"
            });
            mcp::serve(
                &mut *conn,
                mcp::Options {
                    read_only: !allow_writes,
                    max_rows,
                    audit,
                },
            )
            .await?;
            let _ = conn.close().await;
            Ok(std::process::ExitCode::SUCCESS)
        }

        Command::Tables { target, schema } => {
            let mut conn = connect(&target).await?;
            let graph = conn.schema_graph(schema.as_deref()).await?;
            for table in &graph.tables {
                println!("{}", table.name);
            }
            note(&format!("{} tables", graph.tables.len()));
            let _ = conn.close().await;
            Ok(std::process::ExitCode::SUCCESS)
        }
    }
}

// ---------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------

fn driver_for(target: &Target) -> Result<std::sync::Arc<dyn Driver>> {
    let parsed = tablex_core::url::parse(&target.url)?;
    tablex_drivers::registry()
        .get(&parsed.config.driver)
        .map_err(Into::into)
}

async fn connect(target: &Target) -> Result<Box<dyn Connection>> {
    connect_url(&target.url, target.password.as_deref()).await
}

async fn connect_url(url: &str, password: Option<&str>) -> Result<Box<dyn Connection>> {
    let parsed = tablex_core::url::parse(url)?;
    let driver = tablex_drivers::registry().get(&parsed.config.driver)?;
    // An explicit --password wins over one embedded in the URL, so a pipeline
    // can keep the secret out of the URL entirely.
    let secret = password.map(str::to_string).or(parsed.password);

    driver
        .connect(&parsed.config, secret.as_deref())
        .await
        .with_context(|| format!("could not connect to {}", parsed.config.name))
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn open_output(path: Option<&str>) -> Result<Box<dyn Write + Send>> {
    match path {
        Some(path) => Ok(Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).with_context(|| format!("could not create {path}"))?,
        ))),
        None => Ok(Box::new(std::io::BufWriter::new(std::io::stdout()))),
    }
}

fn write_rows(
    out: &mut Box<dyn Write + Send>,
    rows: &tablex_core::result::ResultSet,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Table => write_table(out, rows),
        OutputFormat::Csv => {
            let mut writer =
                tablex_core::export::Writer::new(out, Format::Csv, &rows.columns, "result", '"');
            writer.begin()?;
            writer.write_batch(&rows.rows)?;
            writer.finish()?;
            Ok(())
        }
        OutputFormat::Json => {
            let mut writer =
                tablex_core::export::Writer::new(out, Format::Json, &rows.columns, "result", '"');
            writer.begin()?;
            writer.write_batch(&rows.rows)?;
            writer.finish()?;
            Ok(())
        }
        OutputFormat::Ndjson => {
            for row in &rows.rows {
                let object: serde_json::Map<String, serde_json::Value> = rows
                    .columns
                    .iter()
                    .zip(row)
                    .map(|(column, value)| (column.name.clone(), json_of(value)))
                    .collect();
                writeln!(out, "{}", serde_json::Value::Object(object))?;
            }
            Ok(())
        }
    }
}

/// One value as JSON.
///
/// Exact numerics stay strings rather than becoming JSON numbers: a consumer
/// parsing them as doubles is exactly the rounding this application spends its
/// effort avoiding, and a string is the only JSON type that survives it.
pub(crate) fn json_of(value: &tablex_core::value::Value) -> serde_json::Value {
    use tablex_core::value::Value as V;
    match value {
        V::Null => serde_json::Value::Null,
        V::Bool(b) => serde_json::Value::Bool(*b),
        V::Int(i) => serde_json::Value::from(*i),
        V::UInt(u) => serde_json::Value::from(*u),
        V::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        V::Json(j) => j.clone(),
        other => serde_json::Value::String(other.to_string()),
    }
}

/// Aligned columns, sized to the widest cell.
fn write_table(out: &mut Box<dyn Write + Send>, rows: &tablex_core::result::ResultSet) -> Result<()> {
    let mut widths: Vec<usize> = rows.columns.iter().map(|c| c.name.chars().count()).collect();
    let cells: Vec<Vec<String>> = rows
        .rows
        .iter()
        .map(|row| row.iter().map(|v| v.to_string()).collect())
        .collect();

    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(i) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }

    let line = |out: &mut Box<dyn Write + Send>| -> Result<()> {
        let bars: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        writeln!(out, "{}", bars.join("-+-"))?;
        Ok(())
    };

    let header: Vec<String> = rows
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| pad(&c.name, widths[i]))
        .collect();
    writeln!(out, "{}", header.join(" | "))?;
    line(out)?;

    for row in &cells {
        let padded: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| pad(cell, *widths.get(i).unwrap_or(&0)))
            .collect();
        writeln!(out, "{}", padded.join(" | "))?;
    }

    if rows.truncated {
        // The row cap is not a property of the data, and a table that just
        // stops looks exactly like one that ended.
        writeln!(out, "(truncated — pass --limit to ask for more)")?;
    }
    Ok(())
}

/// Pad by character count rather than byte length, so a non-ASCII value does
/// not throw the column off by the width of its multi-byte characters.
fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    let mut out = String::with_capacity(width);
    out.push_str(text);
    for _ in len..width {
        out.push(' ');
    }
    out
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

async fn import_sql(conn: &mut dyn Connection, path: &str) -> Result<u64> {
    let text = std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?;
    let mut splitter = tablex_core::sql::Splitter::new();
    let mut statements = splitter.push(&text);
    statements.extend(splitter.finish());

    let opts = FetchOptions {
        max_rows: Some(1),
        offset: 0,
        timeout_secs: None,
    };
    let mut applied = 0u64;
    for (index, statement) in statements.iter().enumerate() {
        conn.execute(statement, &opts)
            .await
            .with_context(|| format!("statement {} failed", index + 1))?;
        applied += 1;
    }
    Ok(applied)
}

async fn import_csv(
    conn: &mut dyn Connection,
    path: &str,
    table: &str,
    delimiter: Option<char>,
    has_header: bool,
    quote: char,
) -> Result<u64> {
    let text = std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?;
    let delimiter = delimiter.unwrap_or_else(|| tablex_core::csv::sniff_delimiter(&text));

    let mut reader = tablex_core::csv::CsvReader::new(delimiter);
    let mut records = reader.push(&text);
    records.extend(reader.finish());
    if records.is_empty() {
        return Ok(0);
    }

    // Column names come from the file's header, or from the table itself when
    // there is none — in which case the file's fields are positional and the
    // table's own column order is the only thing they can line up with.
    let detail = conn.table_detail(None, table).await.ok();
    let columns: Vec<String> = if has_header {
        records.remove(0)
    } else {
        detail
            .as_ref()
            .map(|d| d.columns.iter().map(|c| c.name.clone()).collect())
            .ok_or_else(|| anyhow!("--no-header needs the table's columns, which could not be read"))?
    };

    let types: std::collections::HashMap<String, String> = detail
        .map(|d| {
            d.columns
                .into_iter()
                .map(|c| (c.name, c.type_name))
                .collect()
        })
        .unwrap_or_default();

    let quoted: Vec<String> = columns
        .iter()
        .map(|c| tablex_core::sql::quote_ident(c, quote))
        .collect();

    let opts = FetchOptions {
        max_rows: Some(1),
        offset: 0,
        timeout_secs: None,
    };
    let mut inserted = 0u64;

    // Batched for the same reason the desktop importer batches: the round trips
    // dominate, and a statement large enough to be refused helps nobody.
    for chunk in records.chunks(200) {
        let tuples: Vec<String> = chunk
            .iter()
            .map(|record| {
                let values: Vec<String> = columns
                    .iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let field = record.get(i).map(String::as_str).unwrap_or("");
                        let type_name = types.get(name).map(String::as_str).unwrap_or("");
                        tablex_core::csv::literal_for(field, type_name, true)
                    })
                    .collect();
                format!("({})", values.join(", "))
            })
            .collect();

        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}",
            tablex_core::sql::quote_ident(table, quote),
            quoted.join(", "),
            tuples.join(", ")
        );
        conn.execute(&sql, &opts)
            .await
            .context("a batch of rows was refused")?;
        inserted += chunk.len() as u64;
    }

    Ok(inserted)
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

async fn snapshot(
    conn: &mut dyn Connection,
    schema: Option<&str>,
    label: &str,
) -> Result<tablex_core::diff::SchemaSnapshot> {
    let graph = conn.schema_graph(schema).await?;
    let mut tables = Vec::with_capacity(graph.tables.len());

    for table in &graph.tables {
        // A table that vanished between the listing and the read is skipped
        // rather than failing the comparison: one missing table beats no answer.
        if let Ok(detail) = conn.table_detail(table.schema.as_deref(), &table.name).await {
            tables.push(detail);
        }
    }

    Ok(tablex_core::diff::SchemaSnapshot {
        label: label.to_string(),
        tables,
    })
}
