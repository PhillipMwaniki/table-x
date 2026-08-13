//! # tablepro-drivers
//!
//! Concrete [`tablepro_core::Driver`] implementations, one module per database.
//! Each is behind a Cargo feature so a build can ship only the drivers it needs —
//! which keeps binary size down and, on mobile, keeps unlinkable native
//! dependencies out of the build entirely.

use tablepro_core::registry::DriverRegistry;

/// Build the registry of drivers compiled into this binary.
///
/// Each driver registers itself here behind its Cargo feature, so the set of
/// available drivers is decided at compile time rather than discovered at runtime.
pub fn registry() -> DriverRegistry {
    #[allow(unused_mut)]
    let mut reg = DriverRegistry::new();

    // Drivers are added here as they land.

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
