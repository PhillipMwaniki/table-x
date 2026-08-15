//! What a PostgreSQL server is doing right now.
//!
//! `pg_stat_activity` is the one view worth knowing here: it has a row per
//! backend, the statement each is running, and — via `pg_blocking_pids` — who
//! is waiting on whom, which is the fact that turns "the app is slow" into a
//! pid you can act on.

use super::map_err;
use tablex_core::{
    activity::{humanize_bytes, humanize_seconds, ServerActivity, ServerSession, ServerStat},
    error::{Error, Result},
};
use tokio_postgres::Client;

/// Sessions and server counters, read fresh.
pub async fn activity(client: &Client) -> Result<ServerActivity> {
    Ok(ServerActivity {
        sessions: sessions(client).await?,
        stats: stats(client).await?,
    })
}

async fn sessions(client: &Client) -> Result<Vec<ServerSession>> {
    // `datname IS NOT NULL` drops the background workers — checkpointer,
    // autovacuum launcher, walwriter — which are always present, never
    // actionable, and would otherwise be most of a quiet server's list. It
    // works on every version, unlike `backend_type`, which arrived in 10.
    let rows = client
        .query(
            "SELECT pid::text, \
                    usename::text, \
                    COALESCE(host(client_addr), 'local'), \
                    datname::text, \
                    state, \
                    EXTRACT(EPOCH FROM (now() - COALESCE(query_start, backend_start)))::float8, \
                    query, \
                    pid = pg_backend_pid(), \
                    (SELECT string_agg(b::text, ', ') FROM unnest(pg_blocking_pids(pid)) AS b) \
             FROM pg_stat_activity \
             WHERE datname IS NOT NULL \
             ORDER BY (state = 'active') DESC NULLS LAST, query_start",
            &[],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| ServerSession {
            id: r.get(0),
            user: r.get(1),
            client: r.get(2),
            database: r.get(3),
            state: r.get(4),
            seconds: r.get(5),
            query: r
                .get::<_, Option<String>>(6)
                .filter(|q| !q.trim().is_empty()),
            is_self: r.get(7),
            blocked_by: r.get(8),
        })
        .collect())
}

async fn stats(client: &Client) -> Result<Vec<ServerStat>> {
    // One row, so one round trip. The subqueries over pg_stat_database sum
    // across databases deliberately: these are server-wide numbers, and a
    // per-database cache ratio would say less about why the server is busy.
    let row = client
        .query_one(
            "SELECT current_setting('server_version'), \
                    EXTRACT(EPOCH FROM (now() - pg_postmaster_start_time()))::float8, \
                    (SELECT count(*) FROM pg_stat_activity WHERE datname IS NOT NULL)::int8, \
                    current_setting('max_connections'), \
                    (SELECT sum(blks_hit)::float8 \
                       / NULLIF(sum(blks_hit) + sum(blks_read), 0) FROM pg_stat_database), \
                    pg_database_size(current_database())::int8, \
                    (SELECT sum(xact_commit)::int8 FROM pg_stat_database), \
                    (SELECT sum(xact_rollback)::int8 FROM pg_stat_database)",
            &[],
        )
        .await
        .map_err(map_err)?;

    let version: String = row.get(0);
    let uptime: f64 = row.get(1);
    let connections: i64 = row.get(2);
    let max_connections: String = row.get(3);
    let hit_ratio: Option<f64> = row.get(4);
    let size: i64 = row.get(5);
    let commits: Option<i64> = row.get(6);
    let rollbacks: Option<i64> = row.get(7);

    let mut stats = vec![
        ServerStat::new("Version", version),
        ServerStat::new("Uptime", humanize_seconds(uptime)),
        ServerStat::new("Connections", format!("{connections} of {max_connections}")),
        ServerStat::new("Database size", humanize_bytes(size.max(0) as u64)),
    ];
    if let Some(ratio) = hit_ratio {
        stats.push(ServerStat::new(
            "Cache hit ratio",
            format!("{:.1}%", ratio * 100.0),
        ));
    }
    if let (Some(commits), Some(rollbacks)) = (commits, rollbacks) {
        stats.push(ServerStat::new("Commits", commits.to_string()));
        stats.push(ServerStat::new("Rollbacks", rollbacks.to_string()));
    }
    Ok(stats)
}

/// End a backend.
///
/// `pg_terminate_backend` rather than `pg_cancel_backend`: cancelling stops the
/// statement and leaves the session — and its open transaction, and its locks —
/// exactly where they were, which is rarely what someone reaching for this
/// wants.
pub async fn kill(client: &Client, id: &str) -> Result<()> {
    let pid: i32 = id
        .parse()
        .map_err(|_| Error::query(format!("{id} is not a backend pid")))?;
    let row = client
        .query_one("SELECT pg_terminate_backend($1)", &[&pid])
        .await
        .map_err(map_err)?;

    // The function returns false for a pid that is already gone rather than
    // raising, so saying so is the difference between "done" and "nothing
    // happened, and you were not told".
    if !row.get::<_, bool>(0) {
        return Err(Error::query(format!(
            "no backend with pid {pid} is running"
        )));
    }
    Ok(())
}
