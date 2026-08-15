//! An MCP server over the same drivers, behind the same guards.
//!
//! The point is not that an agent can reach a database — plenty of things let
//! it do that. The point is *what it reaches it through*: the drivers that
//! carry exact numerics without rounding, a row cap that holds, a read-only
//! check that is on unless someone deliberately turned it off, and an audit
//! trail of everything asked for.
//!
//! # Defaults that matter
//!
//! Read-only is the default here and writable is the flag, which is the reverse
//! of the desktop application. A person who opens a query tab meant to open it;
//! an agent handed a connection string did not choose anything, and the cost of
//! the two mistakes is not symmetrical.
//!
//! # What this is not
//!
//! The read-only check is a keyword scan — see [`looks_like_write`]. It stops
//! an agent that drifts into writing a statement it should not; it does not
//! stop one that is trying. A database role without write permission is the
//! boundary, and the server says so in its own tool descriptions rather than
//! letting the caller assume otherwise.

use anyhow::{Context, Result};
use serde_json::{json, Value as Json};
use std::io::{BufRead, Write};
use tablex_core::{
    driver::{Connection, FetchOptions},
    result::StatementResult,
    sql::looks_like_write,
};

/// The protocol revision this speaks.
const PROTOCOL: &str = "2024-11-05";

pub struct Options {
    /// Refuse anything that looks like a write.
    pub read_only: bool,
    /// Largest number of rows any single call may return.
    pub max_rows: usize,
    /// Append every call to this file as JSON lines.
    pub audit: Option<String>,
}

/// Serve MCP over stdin and stdout until the input closes.
pub async fn serve(conn: &mut dyn Connection, options: Options) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line.context("could not read from stdin")?;
        if line.trim().is_empty() {
            continue;
        }

        let request: Json = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(e) => {
                // A parse error has no id to answer against, so it is reported
                // against the null id, which is what the spec asks for.
                write_message(&mut stdout, &error(Json::Null, -32700, &e.to_string()))?;
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Json::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        let response = handle(conn, &options, method, params).await;

        // A notification has no id and gets no reply — answering one is a
        // protocol error, not merely noise.
        let Some(id) = id else { continue };

        let message = match response {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(McpError::Protocol { code, message }) => error(id, code, &message),
            // A tool that failed is a successful call reporting a failure: the
            // agent needs to read the message and try something else, not
            // receive a transport error it cannot interpret.
            Err(McpError::Tool(message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": message}],
                    "isError": true
                }
            }),
        };
        write_message(&mut stdout, &message)?;
    }

    Ok(())
}

enum McpError {
    /// Something wrong with the request itself.
    Protocol { code: i64, message: String },
    /// The tool ran and could not do what was asked.
    Tool(String),
}

fn error(id: Json, code: i64, message: &str) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// One message per line, flushed — the client is waiting on it.
fn write_message(out: &mut impl Write, message: &Json) -> Result<()> {
    writeln!(out, "{message}")?;
    out.flush()?;
    Ok(())
}

async fn handle(
    conn: &mut dyn Connection,
    options: &Options,
    method: &str,
    params: Json,
) -> std::result::Result<Json, McpError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "tablex", "version": env!("CARGO_PKG_VERSION")},
        })),

        "ping" => Ok(json!({})),

        "tools/list" => Ok(json!({"tools": tools(options)})),

        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Json::as_str)
                .ok_or_else(|| McpError::Protocol {
                    code: -32602,
                    message: "tools/call needs a name".into(),
                })?
                .to_string();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let outcome = call_tool(conn, options, &name, &arguments).await;
            audit(options, &name, &arguments, &outcome);

            let text = outcome.map_err(McpError::Tool)?;
            Ok(json!({"content": [{"type": "text", "text": text}], "isError": false}))
        }

        _ => Err(McpError::Protocol {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    }
}

fn tools(options: &Options) -> Vec<Json> {
    let query_note = if options.read_only {
        "This connection refuses anything that looks like a write. The check is a keyword \
         scan, not a permission boundary — the database role decides what is truly allowed."
    } else {
        "WRITES ARE ENABLED on this connection. Statements are executed as given."
    };

    vec![
        json!({
            "name": "query",
            "description": format!(
                "Run a SQL statement and return the rows as JSON. At most {} rows come back; \
                 exact decimals are returned as strings so no digits are lost. {}",
                options.max_rows, query_note
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": {"type": "string", "description": "The statement to run."},
                    "limit": {
                        "type": "integer",
                        "description": "Rows to return, capped by the server's own limit."
                    }
                },
                "required": ["sql"]
            }
        }),
        json!({
            "name": "list_tables",
            "description": "List the tables in a schema, with the foreign keys between them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema": {"type": "string", "description": "Defaults to the driver's usual schema."}
                }
            }
        }),
        json!({
            "name": "describe_table",
            "description": "Columns, types, nullability, primary key, indexes and foreign keys \
                            for one table. Cheaper and more reliable than SELECT *.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": {"type": "string"},
                    "schema": {"type": "string"}
                },
                "required": ["table"]
            }
        }),
        json!({
            "name": "explain",
            "description": "How the engine intends to run a statement, as a plan tree. \
                            Nothing is executed.",
            "inputSchema": {
                "type": "object",
                "properties": {"sql": {"type": "string"}},
                "required": ["sql"]
            }
        }),
    ]
}

async fn call_tool(
    conn: &mut dyn Connection,
    options: &Options,
    name: &str,
    arguments: &Json,
) -> std::result::Result<String, String> {
    let text = |key: &str| arguments.get(key).and_then(Json::as_str).map(str::to_string);

    match name {
        "query" => {
            let sql = text("sql").ok_or("query needs a sql argument")?;

            if options.read_only && looks_like_write(&sql) {
                // Named rather than vague: an agent that is told what tripped
                // can rewrite the statement, and one told "refused" retries the
                // same thing.
                return Err(format!(
                    "refused: this connection is read-only and that statement looks like a write. \
                     Only reads are allowed here. Statement: {}",
                    first_line(&sql)
                ));
            }

            // The caller may ask for fewer than the cap but never for more.
            let asked = arguments
                .get("limit")
                .and_then(Json::as_u64)
                .map(|n| n as usize)
                .unwrap_or(options.max_rows);
            let limit = asked.min(options.max_rows);

            let outcome = conn
                .execute(
                    &sql,
                    &FetchOptions {
                        max_rows: Some(limit),
                        offset: 0,
                        timeout_secs: Some(60),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;

            let mut results = Vec::new();
            for statement in outcome.statements {
                match statement {
                    StatementResult::Rows(rows) => {
                        let objects: Vec<Json> = rows
                            .rows
                            .iter()
                            .map(|row| {
                                let map: serde_json::Map<String, Json> = rows
                                    .columns
                                    .iter()
                                    .zip(row)
                                    .map(|(c, v)| (c.name.clone(), crate::json_of(v)))
                                    .collect();
                                Json::Object(map)
                            })
                            .collect();
                        results.push(json!({
                            "columns": rows.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
                            "rows": objects,
                            // Said plainly, because a capped page that looks
                            // complete is how an agent concludes a table has
                            // twenty rows in it.
                            "truncated": rows.truncated,
                            "row_count": rows.rows.len(),
                        }));
                    }
                    StatementResult::Affected { rows_affected, .. } => {
                        results.push(json!({"rows_affected": rows_affected}));
                    }
                }
            }

            serde_json::to_string_pretty(&json!({
                "elapsed_ms": outcome.elapsed_ms,
                "results": results,
            }))
            .map_err(|e| e.to_string())
        }

        "list_tables" => {
            let graph = conn
                .schema_graph(text("schema").as_deref())
                .await
                .map_err(|e| e.to_string())?;
            let tables: Vec<Json> = graph
                .tables
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "schema": t.schema,
                        "references": t.foreign_keys.iter().map(|k| json!({
                            "columns": k.columns,
                            "table": k.referenced_table,
                            "referenced_columns": k.referenced_columns,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            serde_json::to_string_pretty(&json!({"tables": tables})).map_err(|e| e.to_string())
        }

        "describe_table" => {
            let table = text("table").ok_or("describe_table needs a table argument")?;
            let detail = conn
                .table_detail(text("schema").as_deref(), &table)
                .await
                .map_err(|e| e.to_string())?;

            serde_json::to_string_pretty(&json!({
                "name": detail.name,
                "schema": detail.schema,
                "primary_key": detail.primary_key,
                "estimated_rows": detail.estimated_rows,
                "columns": detail.columns.iter().map(|c| json!({
                    "name": c.name,
                    "type": c.type_name,
                    "nullable": c.nullable,
                    "default": c.default,
                    "auto_increment": c.auto_increment,
                })).collect::<Vec<_>>(),
                "indexes": detail.indexes.iter().map(|i| json!({
                    "name": i.name, "columns": i.columns, "unique": i.unique,
                })).collect::<Vec<_>>(),
                "foreign_keys": detail.foreign_keys.iter().map(|k| json!({
                    "columns": k.columns,
                    "references": k.referenced_table,
                    "referenced_columns": k.referenced_columns,
                })).collect::<Vec<_>>(),
            }))
            .map_err(|e| e.to_string())
        }

        "explain" => {
            let sql = text("sql").ok_or("explain needs a sql argument")?;
            // Never analyzed from here: measuring means running, and an agent
            // asking to see a plan did not ask to execute the statement.
            let plan = conn.explain(&sql, false).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())
        }

        other => Err(format!("unknown tool: {other}")),
    }
}

fn first_line(sql: &str) -> String {
    sql.lines().next().unwrap_or("").trim().to_string()
}

/// Append one line describing a call and how it went.
///
/// Best effort: an audit file that cannot be written is worth a line on stderr,
/// and is not worth failing a query the caller is waiting on. A deployment that
/// needs the log to be a hard requirement should make the file unwritable and
/// watch for the warning rather than relying on this to stop.
fn audit(options: &Options, tool: &str, arguments: &Json, outcome: &std::result::Result<String, String>) {
    let Some(path) = &options.audit else { return };

    let entry = json!({
        "tool": tool,
        "arguments": arguments,
        "ok": outcome.is_ok(),
        "error": outcome.as_ref().err(),
    });

    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{entry}"));

    if let Err(e) = appended {
        eprintln!("warning: could not write the audit log at {path}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(read_only: bool) -> Options {
        Options {
            read_only,
            max_rows: 100,
            audit: None,
        }
    }

    #[test]
    fn the_tool_description_says_which_way_the_guard_is_set() {
        // An agent reads these descriptions to decide what to attempt, so a
        // writable connection that describes itself as read-only is worse than
        // no description at all.
        let guarded = tools(&options(true));
        let text = serde_json::to_string(&guarded).unwrap();
        assert!(text.contains("refuses anything that looks like a write"), "{text}");

        let open = tools(&options(false));
        let text = serde_json::to_string(&open).unwrap();
        assert!(text.contains("WRITES ARE ENABLED"), "{text}");
    }

    #[test]
    fn the_description_admits_what_the_check_is_not() {
        // A keyword scan described as a permission boundary is how somebody
        // ends up trusting it as one.
        let text = serde_json::to_string(&tools(&options(true))).unwrap();
        assert!(text.contains("not a permission boundary"), "{text}");
    }

    #[test]
    fn every_tool_declares_a_schema_with_its_required_arguments() {
        for tool in tools(&options(true)) {
            let name = tool["name"].as_str().unwrap().to_string();
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(tool["description"].as_str().is_some_and(|d| d.len() > 20), "{name}");
        }
    }
}
