//! Who exists on a server, and what they are allowed to do.
//!
//! Every engine models this differently — PostgreSQL has roles that may or may
//! not log in, MySQL has user@host pairs, SQL Server separates server logins
//! from database users, ClickHouse has users and roles as distinct things — so
//! the shared shape keeps the two questions that are the same everywhere: who
//! is there, and what can each of them reach.
//!
//! What is deliberately not normalized is the privilege name. `SELECT` means
//! the same thing everywhere, but `BYPASSRLS`, `PROCESS`, and `VIEW SERVER
//! STATE` do not translate, and a lookup table mapping them onto each other
//! would be inventing an equivalence that does not exist. The engine's own word
//! is shown.

use crate::sql::quote_ident;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// Something that logs in.
    User,
    /// Something that is granted to something else.
    Role,
    /// A group of principals, where the engine distinguishes one.
    Group,
}

/// Someone or something that can hold a privilege.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub name: String,
    pub kind: PrincipalKind,
    /// Whether it can open a connection. A PostgreSQL role that cannot is a
    /// group in all but name, and the distinction is the first thing anyone
    /// looks for.
    pub can_login: bool,
    /// Whether it bypasses permission checks entirely — the one attribute
    /// worth surfacing on its own, since it makes every other row moot.
    pub superuser: bool,
    /// Roles this one inherits from.
    pub member_of: Vec<String>,
    /// Whatever else the engine says about it, in the engine's own words.
    pub attributes: Vec<String>,
}

/// One privilege held over one thing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub grantee: String,
    /// The engine's own name for the privilege.
    pub privilege: String,
    /// What it applies to, qualified. `None` means server- or database-wide.
    pub object: Option<String>,
    /// Whether the holder may grant it onward.
    pub grantable: bool,
    /// Whether this is a denial rather than a grant. Only SQL Server has these,
    /// and a DENY that reads as a GRANT is exactly backwards.
    pub denied: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Privileges {
    pub principals: Vec<Principal>,
    pub grants: Vec<Grant>,
    /// Anything the reader needs to know about how complete this is.
    pub notes: Vec<String>,
}

/// The statement that would grant this, in `quote`'s dialect.
pub fn grant_sql(grant: &Grant, quote: char) -> String {
    let target = match &grant.object {
        Some(object) => format!(" ON {object}"),
        None => String::new(),
    };
    let grantable = if grant.grantable {
        " WITH GRANT OPTION"
    } else {
        ""
    };
    format!(
        "GRANT {}{} TO {}{};",
        grant.privilege,
        target,
        quote_ident(&grant.grantee, quote),
        grantable
    )
}

/// The statement that would take it away.
pub fn revoke_sql(grant: &Grant, quote: char) -> String {
    let target = match &grant.object {
        Some(object) => format!(" ON {object}"),
        None => String::new(),
    };
    format!(
        "REVOKE {}{} FROM {};",
        grant.privilege,
        target,
        quote_ident(&grant.grantee, quote)
    )
}

/// Parse one line of MySQL's `SHOW GRANTS` output.
///
/// MySQL has no catalog view of effective privileges — `mysql.user` and
/// `mysql.tables_priv` hold them in a column-per-privilege layout that differs
/// between versions and forks, while `SHOW GRANTS` renders the same information
/// as the statements that would recreate it. Parsing text is the less fragile
/// of the two, which is not a sentence written often.
///
/// The lines look like:
/// ```text
/// GRANT SELECT, INSERT ON `app`.`users` TO `web`@`%`
/// GRANT ALL PRIVILEGES ON *.* TO `root`@`localhost` WITH GRANT OPTION
/// GRANT `readers`@`%` TO `web`@`%`
/// ```
pub fn parse_mysql_grant(line: &str, grantee: &str) -> Vec<Grant> {
    let line = line.trim();
    let Some(rest) = line.strip_prefix("GRANT ") else {
        return Vec::new();
    };

    let grantable = rest.ends_with("WITH GRANT OPTION");

    // A role grant has no ON clause: `GRANT `readers`@`%` TO `web`@`%``.
    let Some(on) = rest.find(" ON ") else {
        let role = rest
            .split(" TO ")
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if role.is_empty() {
            return Vec::new();
        }
        return vec![Grant {
            grantee: grantee.to_string(),
            privilege: format!("ROLE {}", unquote(&role)),
            object: None,
            grantable,
            denied: false,
        }];
    };

    let privileges = &rest[..on];
    let after = &rest[on + 4..];
    let object = after
        .split(" TO ")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();

    // `*.*` is every object on the server, which is not an object.
    let object = (object != "*.*").then_some(object);

    split_privileges(privileges)
        .into_iter()
        .map(|privilege| Grant {
            grantee: grantee.to_string(),
            privilege,
            object: object.clone(),
            grantable,
            denied: false,
        })
        .collect()
}

/// Split a privilege list on commas that are not inside a column list.
///
/// `SELECT (id, name), UPDATE` is two privileges, and splitting naively on
/// commas makes it four — two of which are fragments that are not privileges.
fn split_privileges(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;

    for ch in text.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    out.push(current);

    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Strip MySQL's backticks from an identifier it wrote itself.
///
/// An account name is two quoted parts around an `@` — ``​`web`@`%`​`` — so
/// trimming the outer pair off the whole string leaves the inner backticks
/// stranded in the middle. Each quoted run is unwrapped separately instead, and
/// a doubled backtick inside one is a literal.
fn unquote(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.trim().chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '`' {
            out.push(ch);
            continue;
        }
        // Inside a quoted run until the closing backtick, with `` meaning one.
        while let Some(inner) = chars.next() {
            if inner != '`' {
                out.push(inner);
            } else if chars.peek() == Some(&'`') {
                out.push('`');
                chars.next();
            } else {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_privilege_list_becomes_one_grant_each() {
        let grants = parse_mysql_grant(
            "GRANT SELECT, INSERT, UPDATE ON `app`.`users` TO `web`@`%`",
            "web@%",
        );
        assert_eq!(grants.len(), 3);
        assert_eq!(grants[0].privilege, "SELECT");
        assert_eq!(grants[2].privilege, "UPDATE");
        assert_eq!(grants[0].object.as_deref(), Some("`app`.`users`"));
        assert_eq!(grants[0].grantee, "web@%");
        assert!(!grants[0].grantable);
    }

    #[test]
    fn a_column_list_does_not_split_into_fragments() {
        // Splitting naively on commas makes this four privileges, two of which
        // are not privileges.
        let grants = parse_mysql_grant(
            "GRANT SELECT (id, name), UPDATE (name) ON `app`.`users` TO `web`@`%`",
            "web@%",
        );
        assert_eq!(grants.len(), 2);
        assert_eq!(grants[0].privilege, "SELECT (id, name)");
        assert_eq!(grants[1].privilege, "UPDATE (name)");
    }

    #[test]
    fn everything_everywhere_is_not_an_object() {
        let grants = parse_mysql_grant(
            "GRANT ALL PRIVILEGES ON *.* TO `root`@`localhost` WITH GRANT OPTION",
            "root@localhost",
        );
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].object, None);
        assert!(grants[0].grantable);
    }

    #[test]
    fn a_role_grant_has_no_object_clause_at_all() {
        let grants = parse_mysql_grant("GRANT `readers`@`%` TO `web`@`%`", "web@%");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].privilege, "ROLE readers@%");
        assert_eq!(grants[0].object, None);
    }

    #[test]
    fn a_backtick_inside_a_name_survives_unquoting() {
        // Doubled backticks are how MySQL writes a literal one, and an account
        // name is two quoted runs around an @ rather than one quoted string.
        assert_eq!(unquote("`web`@`%`"), "web@%");
        assert_eq!(unquote("`od``d`@`localhost`"), "od`d@localhost");
        assert_eq!(unquote("plain"), "plain");
    }

    #[test]
    fn a_line_that_is_not_a_grant_yields_nothing() {
        // Rather than a Grant whose privilege is the whole line.
        assert!(parse_mysql_grant("REVOKE SELECT ON x FROM y", "y").is_empty());
        assert!(parse_mysql_grant("", "y").is_empty());
    }

    #[test]
    fn a_database_wide_grant_keeps_its_object() {
        let grants = parse_mysql_grant("GRANT SELECT ON `app`.* TO `web`@`%`", "web@%");
        assert_eq!(grants[0].object.as_deref(), Some("`app`.*"));
    }

    #[test]
    fn grant_and_revoke_are_written_in_the_engines_quoting() {
        let grant = Grant {
            grantee: "web".into(),
            privilege: "SELECT".into(),
            object: Some("app.users".into()),
            grantable: true,
            denied: false,
        };
        assert_eq!(
            grant_sql(&grant, '"'),
            r#"GRANT SELECT ON app.users TO "web" WITH GRANT OPTION;"#
        );
        // Revoking does not take the grant option back separately; taking the
        // privilege takes the right to pass it on with it.
        assert_eq!(
            revoke_sql(&grant, '`'),
            "REVOKE SELECT ON app.users FROM `web`;"
        );
    }

    #[test]
    fn a_server_wide_privilege_is_written_without_an_on_clause() {
        let grant = Grant {
            grantee: "ops".into(),
            privilege: "PROCESS".into(),
            object: None,
            grantable: false,
            denied: false,
        };
        assert_eq!(grant_sql(&grant, '`'), "GRANT PROCESS TO `ops`;");
    }
}
