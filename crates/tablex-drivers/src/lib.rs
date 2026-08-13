//! # tablex-drivers
//!
//! Concrete [`tablex_core::Driver`] implementations, one module per database.
//! Each is behind a Cargo feature so a build can ship only the drivers it needs —
//! which keeps binary size down and, on mobile, keeps unlinkable native
//! dependencies out of the build entirely.

use tablex_core::registry::DriverRegistry;

#[cfg(feature = "clickhouse")]
pub mod clickhouse;
#[cfg(feature = "mssql")]
pub mod mssql;
#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;

/// Build the registry of drivers compiled into this binary.
///
/// Each driver registers itself here behind its Cargo feature, so the set of
/// available drivers is decided at compile time rather than discovered at runtime.
pub fn registry() -> DriverRegistry {
    #[allow(unused_mut)]
    let mut reg = DriverRegistry::new();

    #[cfg(feature = "clickhouse")]
    reg.register(std::sync::Arc::new(clickhouse::ClickhouseDriver::new()));
    #[cfg(feature = "mssql")]
    reg.register(std::sync::Arc::new(mssql::MssqlDriver::new()));
    #[cfg(feature = "mysql")]
    reg.register(std::sync::Arc::new(mysql::MysqlDriver::new()));
    #[cfg(feature = "postgres")]
    reg.register(std::sync::Arc::new(postgres::PostgresDriver::new()));
    #[cfg(feature = "sqlite")]
    reg.register(std::sync::Arc::new(sqlite::SqliteDriver::new()));

    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_builds() {
        // Drivers are registered here as each lands; this guards the wiring itself.
        let _ = registry();
    }
}
