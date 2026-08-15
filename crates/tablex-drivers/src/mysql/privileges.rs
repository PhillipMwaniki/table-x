//! Users and grants on a MySQL or MariaDB server.

use super::map_err;
use mysql_async::{prelude::Queryable, Conn};
use tablex_core::{
    error::Result,
    privileges::{parse_mysql_grant, Principal, PrincipalKind, Privileges},
};

pub async fn privileges(conn: &mut Conn) -> Result<Privileges> {
    // `mysql.user` needs the SELECT privilege on the mysql database, which a
    // non-administrative account does not have. The failure is reported rather
    // than swallowed — an empty user list would read as "this server has no
    // users", which is never true.
    let accounts: Vec<(String, String)> = conn
        .query("SELECT User, Host FROM mysql.user ORDER BY User, Host")
        .await
        .map_err(map_err)?;

    let mut principals = Vec::with_capacity(accounts.len());
    let mut grants = Vec::new();
    let mut notes = Vec::new();

    for (user, host) in accounts {
        let account = format!("{user}@{host}");

        // One SHOW GRANTS per account. There is no bulk form, and the counts
        // here are in the tens rather than the thousands — this is not the
        // shape that made table-by-table introspection worth avoiding.
        let lines: Vec<String> = match conn
            .query(format!(
                "SHOW GRANTS FOR '{}'@'{}'",
                escape(&user),
                escape(&host)
            ))
            .await
        {
            Ok(lines) => lines,
            Err(e) => {
                // A user whose grants cannot be read is still a user; saying
                // which one and why beats dropping it from the list.
                notes.push(format!("Could not read grants for {account}: {e}"));
                Vec::new()
            }
        };

        let mut member_of = Vec::new();
        let mut superuser = false;
        for line in &lines {
            for grant in parse_mysql_grant(line, &account) {
                if let Some(role) = grant.privilege.strip_prefix("ROLE ") {
                    member_of.push(role.to_string());
                    continue;
                }
                // ALL PRIVILEGES with no object is MySQL's superuser: it is
                // every privilege on every object on the server.
                if grant.object.is_none() && grant.privilege.starts_with("ALL PRIVILEGES") {
                    superuser = true;
                }
                grants.push(grant);
            }
        }

        principals.push(Principal {
            name: account,
            // MySQL 8 has roles, and they are accounts that cannot log in —
            // but the catalog does not say which is which in a way MariaDB
            // shares. An account with no password and no login is left as a
            // user rather than guessed at.
            kind: PrincipalKind::User,
            can_login: true,
            superuser,
            member_of,
            attributes: Vec::new(),
        });
    }

    Ok(Privileges {
        principals,
        grants,
        notes,
    })
}

/// `SHOW GRANTS FOR` takes no placeholder, so the account is escaped by hand.
///
/// Both halves come from `mysql.user` rather than from the user, so this is
/// belt and braces — but an account named with a quote would otherwise produce
/// a syntax error nobody could explain.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}
