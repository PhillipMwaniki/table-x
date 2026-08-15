//! Tauri application shell.
//!
//! Intentionally thin: this layer owns process lifetime, the IPC surface, and the
//! OS keychain. All database behaviour lives in `tablex-core` and
//! `tablex-drivers`, which know nothing about Tauri.

mod export;
mod history;
mod import;
mod ipc;
mod secrets;
mod sessions;
mod notebooks;
mod snapshot;
mod snippets;
mod state;
mod store;

use state::AppState;
use tauri::Manager;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(
            EnvFilter::try_from_env("TABLEX_LOG")
                .unwrap_or_else(|_| EnvFilter::new("tablex=info,warn")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // The config directory is only resolvable once the app exists, so
            // state is built here rather than before the builder.
            let config_dir = app.path().app_config_dir()?;
            app.manage(AppState::new(&config_dir));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Close database sockets rather than letting the process exit
                // drop them, so servers see a clean disconnect.
                let state = window.state::<AppState>();
                tauri::async_runtime::block_on(state.sessions.close_all());
            }
        })
        .invoke_handler(tauri::generate_handler![
            ipc::backend_info,
            ipc::list_drivers,
            ipc::list_connections,
            ipc::open_connections,
            ipc::save_connection,
            ipc::delete_connection,
            ipc::connect,
            ipc::test_connection,
            ipc::disconnect,
            ipc::execute,
            ipc::browse,
            ipc::table_detail,
            ipc::explain,
            ipc::schema_diagram,
            ipc::compare_schemas,
            ipc::privileges,
            ipc::server_activity,
            ipc::kill_session,
            ipc::object_definition,
            ipc::export_table,
            ipc::export_database,
            ipc::export_rows,
            ipc::inspect_statement,
            ipc::export_history,
            ipc::import_sql,
            ipc::import_csv,
            ipc::preview_csv,
            ipc::cancel_export,
            ipc::apply_edit,
            ipc::completion_scope,
            ipc::ssh_host_fingerprint,
            ipc::format_sql,
            ipc::list_snippets,
            ipc::save_snippet,
            ipc::delete_snippet,
            ipc::list_notebooks,
            ipc::save_notebook,
            ipc::delete_notebook,
            ipc::query_history,
            ipc::clear_query_history,
            ipc::session_info,
            ipc::use_database,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Table X");
}
