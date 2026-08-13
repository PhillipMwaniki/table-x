//! Tauri application shell.
//!
//! Intentionally thin: this layer owns process lifetime, the IPC surface, and the
//! OS keychain. All database behaviour lives in `tablepro-core` and
//! `tablepro-drivers`, which know nothing about Tauri.

mod ipc;
mod state;

use state::AppState;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(
            EnvFilter::try_from_env("TABLEPRO_LOG")
                .unwrap_or_else(|_| EnvFilter::new("tablepro=info,warn")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            ipc::backend_info,
            ipc::list_drivers
        ])
        .run(tauri::generate_context!())
        .expect("error while running TablePro X");
}
