//! Roles and grants on a PostgreSQL server.

use super::map_err;
use tablex_core::{
    error::Result,
    privileges::{Grant, Principal, PrincipalKind, Privileges},
};
use tokio_postgres::Client;

pub async fn privileges(client: &Client) -> Result<Privileges> {
    Ok(Privileges {
        principals: principals(client).await?,
        grants: grants(client).await?,
        notes: vec![
            // aclexplode over a NULL relacl returns nothing, and NULL means
            // "nobody has been granted anything, so the owner has everything by
            // default". Saying so beats a reader concluding the table is
            // unreachable.
            "Tables with default permissions are not listed: PostgreSQL stores no ACL \
             for them, and their owner holds everything."
                .into(),
        ],
    })
}

async fn principals(client: &Client) -> Result<Vec<Principal>> {
    // `pg_` prefixed roles are the built-in predefined ones — pg_read_all_data
    // and friends. They exist on every server and are noise unless someone was
    // granted one, which shows up as a grant either way.
    let rows = client
        .query(
            "SELECT r.rolname::text, r.rolcanlogin, r.rolsuper, r.rolcreatedb, \
                    r.rolcreaterole, r.rolreplication, r.rolbypassrls, \
                    r.rolvaliduntil IS NOT NULL AND r.rolvaliduntil < now(), \
                    ARRAY(SELECT b.rolname::text FROM pg_catalog.pg_auth_members m \
                          JOIN pg_catalog.pg_roles b ON b.oid = m.roleid \
                          WHERE m.member = r.oid ORDER BY b.rolname) \
             FROM pg_catalog.pg_roles r \
             WHERE r.rolname NOT LIKE 'pg\\_%' \
             ORDER BY r.rolname",
            &[],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let can_login: bool = r.get(1);
            let mut attributes = Vec::new();
            for (flag, label) in [
                (r.get::<_, bool>(3), "CREATEDB"),
                (r.get::<_, bool>(4), "CREATEROLE"),
                (r.get::<_, bool>(5), "REPLICATION"),
                (r.get::<_, bool>(6), "BYPASSRLS"),
            ] {
                if flag {
                    attributes.push(label.to_string());
                }
            }
            // An expired role still exists and still holds its grants; it just
            // cannot use them, which is worth seeing next to the grants.
            if r.get::<_, bool>(7) {
                attributes.push("EXPIRED".into());
            }

            Principal {
                name: r.get(0),
                // A role that cannot log in is a group in all but name, and
                // that distinction is the first thing anyone looks for.
                kind: if can_login {
                    PrincipalKind::User
                } else {
                    PrincipalKind::Role
                },
                can_login,
                superuser: r.get(2),
                member_of: r.get(8),
                attributes,
            }
        })
        .collect())
}

async fn grants(client: &Client) -> Result<Vec<Grant>> {
    // `aclexplode` turns the packed ACL array into one row per privilege, which
    // is the shape everything downstream wants. Reading `relacl` directly is
    // what makes this see grants the current user is neither grantor nor
    // grantee of — information_schema.role_table_grants hides those.
    let rows = client
        .query(
            "SELECT COALESCE(g.grantee::regrole::text, 'PUBLIC'), \
                    n.nspname || '.' || c.relname, \
                    g.privilege_type, \
                    g.is_grantable \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             CROSS JOIN LATERAL aclexplode(c.relacl) g \
             WHERE n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
               AND c.relkind IN ('r', 'p', 'v', 'm') \
             ORDER BY 1, 2, 3",
            &[],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| Grant {
            grantee: r.get(0),
            object: Some(r.get(1)),
            privilege: r.get(2),
            grantable: r.get(3),
            // PostgreSQL has no DENY; absence of a grant is the denial.
            denied: false,
        })
        .collect())
}
