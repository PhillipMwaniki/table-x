//! Principals and permissions in a SQL Server database.
//!
//! SQL Server separates server logins from database users, and this reports the
//! database ones: they are what holds permissions on the tables being looked at,
//! and a connection is scoped to a database anyway.

use super::map_err;
use tablex_core::{
    error::Result,
    privileges::{Grant, Principal, PrincipalKind, Privileges},
};
use tiberius::Client;
use tokio::net::TcpStream;
use tokio_util::compat::Compat;

type Sql = Client<Compat<TcpStream>>;

pub async fn privileges(client: &mut Sql) -> Result<Privileges> {
    Ok(Privileges {
        principals: principals(client).await?,
        grants: grants(client).await?,
        notes: vec![
            "Database principals only. Server-level logins and their roles are separate, \
             and a connection sees one database."
                .into(),
        ],
    })
}

async fn principals(client: &mut Sql) -> Result<Vec<Principal>> {
    // The fixed database roles (db_owner, db_datareader…) are principals like
    // any other and are listed, because membership in one is usually the whole
    // answer to what somebody can do.
    let rows = client
        .simple_query(
            "SELECT p.name, p.type_desc, \
                    STUFF(( SELECT ', ' + r.name \
                            FROM sys.database_role_members m \
                            JOIN sys.database_principals r ON r.principal_id = m.role_principal_id \
                            WHERE m.member_principal_id = p.principal_id \
                            ORDER BY r.name FOR XML PATH('')), 1, 2, '') AS roles \
             FROM sys.database_principals p \
             WHERE p.name NOT LIKE '##%' AND p.principal_id > 0 \
             ORDER BY p.name",
        )
        .await
        .map_err(map_err)?
        .into_first_result()
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let type_desc = r.get::<&str, _>(1).unwrap_or_default().to_string();
            let member_of: Vec<String> = r
                .get::<&str, _>(2)
                .unwrap_or_default()
                .split(", ")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();

            let is_role = type_desc.contains("ROLE");
            Principal {
                name: r.get::<&str, _>(0).unwrap_or_default().to_string(),
                kind: if is_role {
                    PrincipalKind::Role
                } else {
                    PrincipalKind::User
                },
                can_login: !is_role,
                // db_owner is as close as a database principal gets, and
                // membership says it more precisely than a flag would.
                superuser: member_of.iter().any(|r| r == "db_owner"),
                member_of,
                attributes: vec![type_desc],
            }
        })
        .collect())
}

async fn grants(client: &mut Sql) -> Result<Vec<Grant>> {
    // class_desc distinguishes a permission on the database itself from one on
    // an object; OBJECT_SCHEMA_NAME returns NULL for the former, which is
    // exactly the "no object" case the shared shape models.
    let rows = client
        .simple_query(
            "SELECT USER_NAME(dp.grantee_principal_id) AS grantee, \
                    dp.permission_name, \
                    dp.state_desc, \
                    CASE WHEN dp.class = 1 \
                         THEN OBJECT_SCHEMA_NAME(dp.major_id) + '.' + OBJECT_NAME(dp.major_id) \
                    END AS object \
             FROM sys.database_permissions dp \
             WHERE dp.grantee_principal_id > 0 \
             ORDER BY grantee, object, dp.permission_name",
        )
        .await
        .map_err(map_err)?
        .into_first_result()
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let state = r.get::<&str, _>(2).unwrap_or_default();
            Grant {
                grantee: r.get::<&str, _>(0).unwrap_or_default().to_string(),
                privilege: r.get::<&str, _>(1).unwrap_or_default().to_string(),
                object: r.get::<&str, _>(3).map(str::to_string),
                grantable: state == "GRANT_WITH_GRANT_OPTION",
                // A DENY that reads as a GRANT is exactly backwards, and SQL
                // Server's DENY beats any GRANT the principal has by any route.
                denied: state == "DENY",
            }
        })
        .collect())
}
