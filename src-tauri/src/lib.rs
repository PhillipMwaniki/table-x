//! Tauri application shell.
//!
//! Intentionally thin: this layer owns process lifetime, the IPC surface, and the
//! OS keychain. All database behaviour lives in `tablex-core` and
//! `tablex-drivers`, which know nothing about Tauri.

mod designs;
mod export;
mod history;
mod import;
mod ipc;
mod notebooks;
mod secrets;
mod sessions;
mod snapshot;
mod snippets;
mod state;
mod store;
mod update;

use state::AppState;
use tauri::{Emitter, Manager};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// The event a design file arrives on when the app is already running.
pub const OPEN_DESIGN_EVENT: &str = "open-design-file";

/// The design files named on a command line.
///
/// Double-clicking a `.erd` file runs the app with its path as an argument, and
/// so does every other way the shell has of handing a file to a program. The
/// first argument is the executable and is skipped; anything that is not a
/// design file is ignored rather than guessed at, since the same command line
/// carries flags on a development run.
pub fn design_arguments(args: impl Iterator<Item = String>) -> Vec<String> {
    args.skip(1)
        .filter(|arg| {
            std::path::Path::new(arg)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(designs::FILE_EXTENSION))
        })
        .collect()
}

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
        // One instance, because a second one would share this one's files.
        // Every store here is a whole-file write, so two copies of the app
        // editing designs or connections would each save the list they happened
        // to load and the later save would quietly drop the other's work. It is
        // also what makes double-clicking a design open it *here* rather than
        // starting another copy of the application.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let files = design_arguments(args.into_iter());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
                if !files.is_empty() {
                    let _ = window.emit(OPEN_DESIGN_EVENT, files);
                }
            }
        }))
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
            ipc::reconnect,
            ipc::test_connection,
            ipc::disconnect,
            ipc::execute,
            ipc::browse,
            update::check_for_update,
            ipc::table_detail,
            ipc::create_database,
            ipc::create_schema,
            ipc::preview_table_changes,
            ipc::apply_table_changes,
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
            ipc::format_rows,
            ipc::cancel_query,
            ipc::transaction_state,
            ipc::begin_transaction,
            ipc::commit_transaction,
            ipc::rollback_transaction,
            ipc::inspect_statement,
            ipc::export_history,
            ipc::import_sql,
            ipc::import_csv,
            ipc::preview_csv,
            ipc::cancel_export,
            ipc::apply_edit,
            ipc::insert_row,
            ipc::delete_row,
            ipc::completion_scope,
            ipc::ssh_host_fingerprint,
            ipc::format_sql,
            ipc::list_snippets,
            ipc::save_snippet,
            ipc::delete_snippet,
            ipc::list_designs,
            ipc::write_design_file,
            ipc::read_design_file,
            ipc::startup_designs,
            ipc::save_design,
            ipc::delete_design,
            ipc::design_diagram,
            ipc::design_from_schema,
            ipc::design_script,
            ipc::design_sync,
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

#[cfg(test)]
mod tests {
    use super::design_arguments;

    fn args(list: &[&str]) -> Vec<String> {
        design_arguments(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn a_double_clicked_design_is_the_argument_after_the_executable() {
        assert_eq!(
            args(&["table-x.exe", "C:/work/shop.erd"]),
            vec!["C:/work/shop.erd".to_string()]
        );
    }

    #[test]
    fn the_executable_is_never_mistaken_for_a_file_to_open() {
        // Not hypothetical: the first argument is always there, and a filter
        // that forgot to skip it would try to open the program itself.
        assert!(args(&["table-x.exe"]).is_empty());
    }

    #[test]
    fn anything_that_is_not_a_design_is_left_alone() {
        // A development run carries flags on the same command line, and the
        // shell hands over whatever it was given.
        assert!(args(&["table-x.exe", "--no-default-features", "notes.txt"]).is_empty());
    }

    #[test]
    fn the_extension_is_matched_however_it_was_typed() {
        assert_eq!(args(&["t.exe", "Shop.ERD"]), vec!["Shop.ERD".to_string()]);
    }

    #[test]
    fn several_files_at_once_all_arrive() {
        assert_eq!(args(&["t.exe", "a.erd", "b.erd"]).len(), 2);
    }
}
