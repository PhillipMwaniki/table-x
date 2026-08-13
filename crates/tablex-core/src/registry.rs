//! Driver registry.
//!
//! Maps a driver id from a saved connection to the implementation that handles it.
//! Kept in core (rather than in the driver crate) so that the app, the test suite,
//! and any future plugin host all resolve drivers the same way.

use crate::{driver::Driver, driver::DriverInfo, error::Error, error::Result};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct DriverRegistry {
    drivers: BTreeMap<String, Arc<dyn Driver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a driver under its own declared id.
    pub fn register(&mut self, driver: Arc<dyn Driver>) {
        let id = driver.info().id;
        self.drivers.insert(id, driver);
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Driver>> {
        self.drivers
            .get(id)
            .cloned()
            .ok_or_else(|| Error::UnknownDriver(id.to_string()))
    }

    /// Every registered driver, for the "new connection" picker.
    /// Sorted by display name so the list is stable between launches.
    pub fn list(&self) -> Vec<DriverInfo> {
        let mut infos: Vec<DriverInfo> = self.drivers.values().map(|d| d.info()).collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    pub fn contains(&self, id: &str) -> bool {
        self.drivers.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.drivers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ConnectionConfig,
        driver::{Capabilities, Connection},
    };
    use async_trait::async_trait;

    struct Fake(&'static str, &'static str);

    #[async_trait]
    impl Driver for Fake {
        fn info(&self) -> DriverInfo {
            DriverInfo {
                id: self.0.into(),
                name: self.1.into(),
                default_port: None,
                file_based: true,
                capabilities: Capabilities::default(),
            }
        }
        async fn connect(
            &self,
            _c: &ConnectionConfig,
            _s: Option<&str>,
        ) -> Result<Box<dyn Connection>> {
            Err(Error::Other("not used in this test".into()))
        }
    }

    #[test]
    fn unknown_driver_is_a_named_error_not_a_panic() {
        let reg = DriverRegistry::new();
        // A config file naming a driver this build does not ship must fail
        // gracefully — users copy configs between machines and versions.
        match reg.get("nope") {
            Err(Error::UnknownDriver(id)) => assert_eq!(id, "nope"),
            Err(other) => panic!("expected UnknownDriver, got {other:?}"),
            Ok(_) => panic!("expected UnknownDriver, got a driver"),
        }
    }

    #[test]
    fn drivers_register_under_their_declared_id() {
        let mut reg = DriverRegistry::new();
        reg.register(Arc::new(Fake("postgres", "PostgreSQL")));
        assert!(reg.contains("postgres"));
        assert!(reg.get("postgres").is_ok());
    }

    #[test]
    fn listing_is_sorted_by_display_name() {
        let mut reg = DriverRegistry::new();
        reg.register(Arc::new(Fake("sqlite", "SQLite")));
        reg.register(Arc::new(Fake("postgres", "PostgreSQL")));
        reg.register(Arc::new(Fake("mysql", "MySQL")));

        let names: Vec<String> = reg.list().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["MySQL", "PostgreSQL", "SQLite"]);
    }
}
