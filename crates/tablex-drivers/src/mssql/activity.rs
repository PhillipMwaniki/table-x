//! What a SQL Server instance is doing right now.
//!
//! `sys.dm_exec_sessions` has a row per connection and `sys.dm_exec_requests`
//! has one per statement actually executing; joining them is what separates a
//! connection that is idle from one that is working, and
//! `blocking_session_id` is the column that names a blocker outright.

use super::map_err;
use tablex_core::{
    activity::{humanize_seconds, ServerActivity, ServerSession, ServerStat},
    error::{Error, Result},
};
use tiberius::Client;
use tokio::net::TcpStream;
use tokio_util::compat::Compat;

type Sql = Client<Compat<TcpStream>>;

pub async fn activity(client: &mut Sql) -> Result<ServerActivity> {
    Ok(ServerActivity {
        sessions: sessions(client).await?,
        stats: stats(client).await?,
    })
}

async fn sessions(client: &mut Sql) -> Result<Vec<ServerSession>> {
    // OUTER APPLY rather than a join: `dm_exec_sql_text` is a function over the
    // handle, and a session with no request has no handle — an inner join would
    // silently drop every idle connection, which is half of what is being
    // looked at.
    //
    // is_user_process excludes the instance's own background tasks, which are
    // always there and never the answer.
    let rows = client
        .simple_query(
            "SELECT CAST(s.session_id AS varchar(20)) AS id, \
                    s.login_name, \
                    COALESCE(NULLIF(s.host_name, ''), 'local') AS host, \
                    DB_NAME(COALESCE(r.database_id, s.database_id)) AS db, \
                    COALESCE(r.status, s.status) AS state, \
                    CAST(COALESCE(r.total_elapsed_time / 1000.0, 0) AS float) AS seconds, \
                    t.text AS query, \
                    CASE WHEN s.session_id = @@SPID THEN 1 ELSE 0 END AS is_self, \
                    CASE WHEN r.blocking_session_id > 0 \
                         THEN CAST(r.blocking_session_id AS varchar(20)) END AS blocked_by \
             FROM sys.dm_exec_sessions s \
             LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id \
             OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t \
             WHERE s.is_user_process = 1 \
             ORDER BY CASE WHEN r.session_id IS NULL THEN 1 ELSE 0 END, \
                      r.total_elapsed_time DESC",
        )
        .await
        .map_err(map_err)?
        .into_first_result()
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let text = |i: usize| r.get::<&str, _>(i).map(str::to_string);
            ServerSession {
                id: text(0).unwrap_or_default(),
                user: text(1),
                client: text(2),
                database: text(3),
                state: text(4),
                seconds: r.get::<f64, _>(5),
                query: text(6).filter(|q| !q.trim().is_empty()),
                is_self: r.get::<i32, _>(7).unwrap_or(0) != 0,
                blocked_by: text(8),
            }
        })
        .collect())
}

async fn stats(client: &mut Sql) -> Result<Vec<ServerStat>> {
    let rows = client
        .simple_query(
            "SELECT CAST(SERVERPROPERTY('ProductVersion') AS varchar(50)) AS version, \
                    CAST(SERVERPROPERTY('Edition') AS varchar(100)) AS edition, \
                    CAST(DATEDIFF(SECOND, si.sqlserver_start_time, GETDATE()) AS float) AS uptime, \
                    (SELECT COUNT(*) FROM sys.dm_exec_sessions WHERE is_user_process = 1) AS conns, \
                    (SELECT COUNT(*) FROM sys.dm_exec_requests \
                      WHERE blocking_session_id > 0) AS blocked \
             FROM sys.dm_os_sys_info si",
        )
        .await
        .map_err(map_err)?
        .into_first_result()
        .await
        .map_err(map_err)?;

    let Some(row) = rows.first() else {
        return Ok(Vec::new());
    };

    let mut stats = Vec::new();
    if let Some(version) = row.get::<&str, _>(0) {
        stats.push(ServerStat::new("Version", version));
    }
    if let Some(edition) = row.get::<&str, _>(1) {
        stats.push(ServerStat::new("Edition", edition));
    }
    if let Some(uptime) = row.get::<f64, _>(2) {
        stats.push(ServerStat::new("Uptime", humanize_seconds(uptime)));
    }
    if let Some(conns) = row.get::<i32, _>(3) {
        stats.push(ServerStat::new("Connections", conns.to_string()));
    }
    // Zero is worth printing here. "Blocked: 0" answers the question someone
    // opened this panel to ask; a missing line leaves them still asking it.
    if let Some(blocked) = row.get::<i32, _>(4) {
        stats.push(ServerStat::new("Blocked requests", blocked.to_string()));
    }
    Ok(stats)
}

/// End a session.
///
/// `KILL` accepts no parameter, so the spid is parsed as a number first — the
/// only form it can legitimately take, and the only validation needed.
pub async fn kill(client: &mut Sql, id: &str) -> Result<()> {
    let spid: i32 = id
        .parse()
        .map_err(|_| Error::query(format!("{id} is not a session id")))?;
    client
        .simple_query(format!("KILL {spid}"))
        .await
        .map_err(map_err)?
        .into_results()
        .await
        .map_err(map_err)?;
    Ok(())
}
