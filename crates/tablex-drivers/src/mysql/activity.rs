//! What a MySQL or MariaDB server is doing right now.

use super::map_err;
use mysql_async::{prelude::Queryable, Conn};
use tablex_core::{
    activity::{humanize_seconds, ServerActivity, ServerSession, ServerStat},
    error::{Error, Result},
};

/// One row of `information_schema.PROCESSLIST`, everything cast to a shape that
/// decodes the same on both engines and every version.
type ProcessRow = (
    String,         // ID
    Option<String>, // USER
    Option<String>, // HOST
    Option<String>, // DB
    Option<String>, // COMMAND
    Option<String>, // STATE
    Option<i64>,    // TIME
    Option<String>, // INFO
    i64,            // whether it is this connection
);

pub async fn activity(conn: &mut Conn) -> Result<ServerActivity> {
    Ok(ServerActivity {
        sessions: sessions(conn).await?,
        stats: stats(conn).await?,
    })
}

async fn sessions(conn: &mut Conn) -> Result<Vec<ServerSession>> {
    // `information_schema.PROCESSLIST` rather than
    // `performance_schema.processlist`: the latter is what MySQL 8 prefers and
    // what MariaDB does not have. This one is present on both, and the columns
    // have been the same for fifteen years.
    //
    // Everything is cast because the raw column types differ across versions —
    // ID is unsigned on one engine and not the other — and a decode failure
    // here would take out the whole panel.
    let rows: Vec<ProcessRow> = conn
        .query(
            "SELECT CAST(ID AS CHAR), USER, HOST, DB, COMMAND, STATE, \
                    CAST(TIME AS SIGNED), INFO, ID = CONNECTION_ID() \
             FROM information_schema.PROCESSLIST \
             ORDER BY (COMMAND <> 'Sleep') DESC, TIME DESC",
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .into_iter()
        .map(
            |(id, user, host, db, command, state, time, info, is_self)| ServerSession {
                id,
                user,
                client: host,
                database: db,
                // MySQL splits what one word says elsewhere: COMMAND is `Query` or
                // `Sleep`, STATE is the phase within it. Both matter — `Query` says
                // it is working, `Sending data` says on what — so they are joined
                // rather than one being picked.
                state: match (command, state) {
                    (Some(c), Some(s)) if !s.is_empty() => Some(format!("{c} — {s}")),
                    (command, state) => command.or(state),
                },
                seconds: time.map(|t| t.max(0) as f64),
                query: info.filter(|q| !q.trim().is_empty()),
                is_self: is_self != 0,
                // No portable answer. MySQL 8 can name a blocker through
                // `performance_schema.data_lock_waits`, MariaDB cannot, and on
                // MySQL it needs performance_schema enabled — so rather than a
                // column that is empty for reasons the user cannot see, there is
                // no column.
                blocked_by: None,
            },
        )
        .collect())
}

async fn stats(conn: &mut Conn) -> Result<Vec<ServerStat>> {
    // `SHOW GLOBAL STATUS` returns several hundred rows, which is a few
    // kilobytes and the only form that reads the same on MySQL 5.7, MySQL 8,
    // and MariaDB — the underlying tables moved between schemas twice.
    let status: Vec<(String, String)> = conn.query("SHOW GLOBAL STATUS").await.map_err(map_err)?;
    let pick = |name: &str| {
        status
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };

    let (version, max_connections): (String, String) = conn
        .query_first("SELECT VERSION(), CAST(@@max_connections AS CHAR)")
        .await
        .map_err(map_err)?
        .unwrap_or_else(|| ("unknown".into(), "?".into()));

    let mut stats = vec![ServerStat::new("Version", version)];
    if let Some(uptime) = pick("Uptime").and_then(|v| v.parse::<f64>().ok()) {
        stats.push(ServerStat::new("Uptime", humanize_seconds(uptime)));
    }
    if let Some(connected) = pick("Threads_connected") {
        stats.push(ServerStat::new(
            "Connections",
            format!("{connected} of {max_connections}"),
        ));
    }
    for (label, name) in [
        ("Running threads", "Threads_running"),
        ("Queries", "Questions"),
        ("Slow queries", "Slow_queries"),
        ("Aborted connections", "Aborted_connects"),
    ] {
        if let Some(value) = pick(name) {
            stats.push(ServerStat::new(label, value));
        }
    }
    Ok(stats)
}

/// End a session.
///
/// `KILL` takes no placeholder, so the id is parsed as a number before it is
/// formatted in — which is also the only validation that matters, since a
/// thread id is the one thing it can legitimately be.
pub async fn kill(conn: &mut Conn, id: &str) -> Result<()> {
    let thread: u64 = id
        .parse()
        .map_err(|_| Error::query(format!("{id} is not a thread id")))?;
    conn.query_drop(format!("KILL {thread}"))
        .await
        .map_err(map_err)
}
