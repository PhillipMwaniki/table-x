//! What the server is doing right now.
//!
//! The question this answers is the one you ask when something is slow and you
//! do not yet know why: who is connected, what are they running, how long have
//! they been running it, and is anyone stuck behind anyone else.

use serde::{Deserialize, Serialize};

/// One session the server reports as connected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSession {
    /// The engine's own identifier for the session, and what a kill is
    /// addressed to. A string because engines disagree: MySQL uses an integer
    /// id, ClickHouse a query UUID.
    pub id: String,
    pub user: Option<String>,
    /// Where the session connected from.
    pub client: Option<String>,
    pub database: Option<String>,
    /// The engine's own word — `active`, `idle in transaction`, `Sleep` —
    /// rather than one normalized across engines. The words mean different
    /// things and flattening them would lose exactly the detail being looked
    /// for.
    pub state: Option<String>,
    /// Seconds spent on the current statement, or in the current state when
    /// nothing is running.
    pub seconds: Option<f64>,
    /// The statement, as the server has it.
    pub query: Option<String>,
    /// Whether this is the app's own session.
    ///
    /// Worth knowing before a kill: ending it disconnects the user from the
    /// tool they are using to end it, which is a surprising way to find out.
    pub is_self: bool,
    /// The session holding the lock this one is waiting on, where the engine
    /// can say. This is the field that turns "everything is slow" into a name.
    pub blocked_by: Option<String>,
}

/// One named server measurement.
///
/// Free-form pairs rather than a struct of fields, because engines measure
/// different things: PostgreSQL has a cache hit ratio and no `Questions`
/// counter, MySQL the reverse. A common struct would be mostly `None` on every
/// engine and would render as a wall of blanks. Each driver reports what it can
/// actually answer, in the order that reads best for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStat {
    pub label: String,
    pub value: String,
}

impl ServerStat {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        ServerStat {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// Everything the activity view shows for one server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerActivity {
    pub sessions: Vec<ServerSession>,
    pub stats: Vec<ServerStat>,
}

/// Render a duration in seconds the way someone scanning a list reads it.
///
/// The precision that matters shifts with the magnitude: a query 400ms in is
/// unremarkable and one that has been running four hours is the answer, so
/// sub-second times keep their decimals and long ones lose them entirely.
pub fn humanize_seconds(seconds: f64) -> String {
    if seconds < 1.0 {
        return format!("{}ms", (seconds * 1000.0).round() as i64);
    }
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let total = seconds.round() as i64;
    let (m, s) = (total / 60, total % 60);
    if m < 60 {
        return format!("{m}m {s}s");
    }
    let (h, m) = (m / 60, m % 60);
    if h < 24 {
        return format!("{h}h {m}m");
    }
    format!("{}d {}h", h / 24, h % 24)
}

/// Render a byte count for a stat line.
pub fn humanize_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_durations_keep_the_precision_that_distinguishes_them() {
        assert_eq!(humanize_seconds(0.4), "400ms");
        assert_eq!(humanize_seconds(2.25), "2.2s");
    }

    #[test]
    fn long_durations_drop_precision_nobody_reads() {
        // Four hours in, the seconds are not the point.
        assert_eq!(humanize_seconds(90.0), "1m 30s");
        assert_eq!(humanize_seconds(3_600.0), "1h 0m");
        assert_eq!(humanize_seconds(14_400.0), "4h 0m");
        assert_eq!(humanize_seconds(180_000.0), "2d 2h");
    }

    #[test]
    fn bytes_step_up_at_a_full_unit() {
        assert_eq!(humanize_bytes(512), "512 B");
        assert_eq!(humanize_bytes(1024), "1.0 KB");
        assert_eq!(humanize_bytes(1_572_864), "1.5 MB");
    }
}
