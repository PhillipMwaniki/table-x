//! Process-wide application state.

use tablepro_core::registry::DriverRegistry;

pub struct AppState {
    /// Drivers compiled into this build.
    pub registry: DriverRegistry,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            registry: tablepro_drivers::registry(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
