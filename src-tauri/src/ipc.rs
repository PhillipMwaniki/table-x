//! Tauri IPC surface.
//!
//! Commands are thin: validate, delegate to core, map errors to a serializable
//! payload. No database logic lives here.

use serde::Serialize;
use tablepro_core::{driver::DriverInfo, ErrorPayload};

use crate::state::AppState;

/// Every fallible command returns this so the frontend has one error shape to
/// handle. The handshake commands below cannot fail, so nothing uses it yet.
#[allow(dead_code)]
pub type IpcResult<T> = std::result::Result<T, ErrorPayload>;

#[derive(Serialize)]
pub struct BackendInfo {
    pub version: String,
    pub drivers: Vec<String>,
}

/// Handshake used by the frontend on boot to confirm the backend is alive and
/// to learn which drivers this build actually ships.
#[tauri::command]
pub fn backend_info(state: tauri::State<'_, AppState>) -> BackendInfo {
    BackendInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        drivers: state.registry.list().into_iter().map(|d| d.name).collect(),
    }
}

/// Full driver descriptors, used to render the connection form for a chosen driver.
#[tauri::command]
pub fn list_drivers(state: tauri::State<'_, AppState>) -> Vec<DriverInfo> {
    state.registry.list()
}
