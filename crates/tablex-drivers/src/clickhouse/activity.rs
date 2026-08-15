//! What a ClickHouse server is doing right now.
//!
//! ClickHouse has no notion of an idle session over HTTP — every request is its
//! own thing — so `system.processes` lists running queries rather than
//! connections. That is the more useful list anyway: on ClickHouse the question
//! is almost always which query is eating the machine.

use tablex_core::{
    activity::{humanize_bytes, humanize_seconds, ServerSession, ServerStat},
    error::{Error, Result},
};

/// Parse the string rows a ClickHouse catalog query returns into sessions.
pub fn sessions_from(rows: Vec<Vec<String>>) -> Vec<ServerSession> {
    rows.into_iter()
        .map(|r| {
            let at = |i: usize| r.get(i).cloned().filter(|v| !v.is_empty());
            ServerSession {
                id: at(0).unwrap_or_default(),
                user: at(1),
                client: at(2),
                database: at(3),
                // Everything in this table is running by definition; the state
                // worth showing is how much memory it has taken to get here.
                state: at(6)
                    .and_then(|b| b.parse::<u64>().ok())
                    .map(|b| format!("running — {} used", humanize_bytes(b)))
                    .or_else(|| Some("running".into())),
                seconds: at(4).and_then(|s| s.parse::<f64>().ok()),
                query: at(5),
                // Nothing here is this app's own session in a way that survives
                // the request: see the filter in `SESSIONS_SQL`.
                is_self: false,
                // ClickHouse queries do not block on each other's locks; they
                // compete for memory and CPU, which the state column shows.
                blocked_by: None,
            }
        })
        .collect()
}

/// The listing query excludes itself.
///
/// Over HTTP each request is its own query, so the statement below is running
/// while it reads the table and appears in its own results — at the top, every
/// refresh, forever. Excluding it costs the ability to see someone else's
/// genuine query against `system.processes`, which is a fair trade for a panel
/// that is not permanently reporting itself.
pub const SESSIONS_SQL: &str = "SELECT query_id, user, toString(address), `database`, \
     toString(elapsed), query, toString(memory_usage) \
     FROM system.processes \
     WHERE query NOT LIKE '%system.processes%' \
     ORDER BY elapsed DESC";

pub const STATS_SQL: &str = "SELECT version(), \
     toString(toUInt64(uptime())), \
     toString((SELECT value FROM system.metrics WHERE metric = 'Query')), \
     toString((SELECT value FROM system.metrics WHERE metric = 'HTTPConnection')), \
     toString((SELECT value FROM system.metrics WHERE metric = 'MemoryTracking'))";

/// Turn the one-row stats result into labelled lines.
pub fn stats_from(rows: Vec<Vec<String>>) -> Vec<ServerStat> {
    let Some(row) = rows.into_iter().next() else {
        return Vec::new();
    };
    let at = |i: usize| row.get(i).cloned().filter(|v| !v.is_empty());

    let mut stats = Vec::new();
    if let Some(version) = at(0) {
        stats.push(ServerStat::new("Version", version));
    }
    if let Some(uptime) = at(1).and_then(|v| v.parse::<f64>().ok()) {
        stats.push(ServerStat::new("Uptime", humanize_seconds(uptime)));
    }
    if let Some(queries) = at(2) {
        stats.push(ServerStat::new("Running queries", queries));
    }
    if let Some(conns) = at(3) {
        stats.push(ServerStat::new("HTTP connections", conns));
    }
    if let Some(memory) = at(4).and_then(|v| v.parse::<u64>().ok()) {
        stats.push(ServerStat::new("Memory tracked", humanize_bytes(memory)));
    }
    stats
}

/// The statement that ends a query, or an error if the id could not be one.
///
/// `KILL QUERY` takes a `WHERE` over the same table and no placeholders, so the
/// id goes in as text. A ClickHouse query id is server-generated and shaped
/// like a UUID; anything that is not is not an id, and refusing it here is
/// simpler and stricter than escaping it.
pub fn kill_sql(id: &str) -> Result<String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::query(format!("{id} is not a query id")));
    }
    Ok(format!("KILL QUERY WHERE query_id = '{id}' SYNC"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_id_that_could_carry_sql_is_refused() {
        assert!(kill_sql("abc' OR '1'='1").is_err());
        assert!(kill_sql("").is_err());
        assert!(kill_sql("a1b2-c3d4_e5").is_ok());
    }

    #[test]
    fn memory_shows_up_in_the_state_because_that_is_what_competes() {
        let sessions = sessions_from(vec![vec![
            "q1".into(),
            "default".into(),
            "10.0.0.1".into(),
            "app".into(),
            "12.5".into(),
            "SELECT 1".into(),
            "1048576".into(),
        ]]);
        assert_eq!(sessions[0].seconds, Some(12.5));
        assert_eq!(sessions[0].state.as_deref(), Some("running — 1.0 MB used"));
    }

    #[test]
    fn a_short_row_does_not_panic() {
        // Catalog shapes change between versions; a missing trailing column
        // should cost that field, not the panel.
        let sessions = sessions_from(vec![vec!["q1".into()]]);
        assert_eq!(sessions[0].id, "q1");
        assert_eq!(sessions[0].seconds, None);
    }
}
