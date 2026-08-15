//! Users, roles, and grants on a ClickHouse server.
//!
//! ClickHouse keeps all three in `system.*` tables, so unlike the other engines
//! here nothing needs parsing — the catalog is already the shape this wants.

use tablex_core::privileges::{Grant, Principal, PrincipalKind, Privileges};

pub const USERS_SQL: &str = "SELECT name, 'user' FROM system.users \
     UNION ALL SELECT name, 'role' FROM system.roles ORDER BY 1";

/// Role membership, one row per (grantee, role).
pub const ROLE_GRANTS_SQL: &str =
    "SELECT user_name, role_name, granted_role_name FROM system.role_grants";

/// `access_type` is the privilege; the database and table columns are empty for
/// a server-wide grant, which is the same thing the shared shape calls `None`.
pub const GRANTS_SQL: &str = "SELECT user_name, role_name, access_type, \
     `database`, `table`, grant_option FROM system.grants";

/// Assemble the three result sets into the shared shape.
pub fn assemble(
    users: Vec<Vec<String>>,
    role_grants: Vec<Vec<String>>,
    grants: Vec<Vec<String>>,
) -> Privileges {
    let at = |row: &[String], i: usize| row.get(i).cloned().unwrap_or_default();

    let mut principals: Vec<Principal> = users
        .iter()
        .map(|row| Principal {
            name: at(row, 0),
            kind: if at(row, 1) == "role" {
                PrincipalKind::Role
            } else {
                PrincipalKind::User
            },
            can_login: at(row, 1) != "role",
            superuser: false,
            member_of: Vec::new(),
            attributes: Vec::new(),
        })
        .collect();

    for row in &role_grants {
        // A role grant names either a user or a role, never both — whichever
        // column is populated is the grantee.
        let holder = if at(row, 0).is_empty() {
            at(row, 1)
        } else {
            at(row, 0)
        };
        if let Some(principal) = principals.iter_mut().find(|p| p.name == holder) {
            principal.member_of.push(at(row, 2));
        }
    }

    let grants: Vec<Grant> = grants
        .iter()
        .map(|row| {
            let holder = if at(row, 0).is_empty() {
                at(row, 1)
            } else {
                at(row, 0)
            };
            let database = at(row, 3);
            let table = at(row, 4);
            let object = match (database.is_empty(), table.is_empty()) {
                (true, _) => None,
                (false, true) => Some(format!("{database}.*")),
                (false, false) => Some(format!("{database}.{table}")),
            };
            Grant {
                grantee: holder,
                privilege: at(row, 2),
                object,
                grantable: at(row, 5) == "1",
                denied: false,
            }
        })
        .collect();

    // ACCESS MANAGEMENT is the closest thing to a superuser flag here, and it
    // is a grant rather than an attribute — so it is read back off the grants.
    for principal in &mut principals {
        principal.superuser = has_full_access(&grants, &principal.name);
    }

    Privileges {
        principals,
        grants,
        notes: Vec::new(),
    }
}

fn has_full_access(grants: &[Grant], name: &str) -> bool {
    grants
        .iter()
        .any(|g| g.grantee == name && g.object.is_none() && g.privilege == "ALL")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn users_and_roles_are_both_principals_and_are_distinguished() {
        let p = assemble(
            vec![row(&["web", "user"]), row(&["readers", "role"])],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(p.principals[0].kind, PrincipalKind::User);
        assert!(p.principals[0].can_login);
        assert_eq!(p.principals[1].kind, PrincipalKind::Role);
        assert!(!p.principals[1].can_login);
    }

    #[test]
    fn a_role_grant_lands_on_whichever_column_names_the_holder() {
        // system.role_grants populates user_name or role_name, never both.
        let p = assemble(
            vec![row(&["web", "user"]), row(&["writers", "role"])],
            vec![row(&["web", "", "readers"]), row(&["", "writers", "readers"])],
            Vec::new(),
        );
        assert_eq!(p.principals[0].member_of, vec!["readers"]);
        assert_eq!(p.principals[1].member_of, vec!["readers"]);
    }

    #[test]
    fn an_empty_database_column_means_the_whole_server() {
        let p = assemble(
            vec![row(&["web", "user"])],
            Vec::new(),
            vec![
                row(&["web", "", "SELECT", "", "", "0"]),
                row(&["web", "", "SELECT", "app", "", "0"]),
                row(&["web", "", "INSERT", "app", "users", "1"]),
            ],
        );
        assert_eq!(p.grants[0].object, None);
        assert_eq!(p.grants[1].object.as_deref(), Some("app.*"));
        assert_eq!(p.grants[2].object.as_deref(), Some("app.users"));
        assert!(p.grants[2].grantable);
    }

    #[test]
    fn full_access_is_read_back_off_the_grants() {
        // ClickHouse has no superuser flag; ALL on everything is what it means.
        let p = assemble(
            vec![row(&["admin", "user"]), row(&["web", "user"])],
            Vec::new(),
            vec![
                row(&["admin", "", "ALL", "", "", "1"]),
                row(&["web", "", "SELECT", "app", "", "0"]),
            ],
        );
        assert!(p.principals[0].superuser);
        assert!(!p.principals[1].superuser);
    }

    #[test]
    fn a_short_row_does_not_panic() {
        let p = assemble(vec![row(&["web"])], Vec::new(), vec![row(&["web"])]);
        assert_eq!(p.principals[0].name, "web");
        assert_eq!(p.grants[0].privilege, "");
    }
}
