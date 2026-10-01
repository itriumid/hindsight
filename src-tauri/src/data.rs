//! "Remove all Hindsight data": everything Hindsight keeps for itself, never the clips someone
//! saved. The removal itself runs as the app exits, after its window has closed, so the web view
//! can't write anything back afterwards.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

/// Set when the person asked to remove everything; acted on at exit.
#[derive(Default)]
pub struct RemoveOnExit(pub AtomicBool);

/// Every folder Hindsight writes to on its own, deduplicated: settings, caches, the web view's
/// storage. Clips live wherever the person chose and are never in here.
pub fn data_folders(app: &AppHandle) -> Vec<PathBuf> {
    let paths = app.path();
    let mut folders: Vec<PathBuf> = [
        paths.app_config_dir(),
        paths.app_data_dir(),
        paths.app_local_data_dir(),
        paths.app_cache_dir(),
        paths.app_log_dir(),
    ]
    .into_iter()
    .flatten()
    .collect();
    #[cfg(target_os = "macos")]
    if let Ok(home) = paths.home_dir() {
        let identifier = &app.config().identifier;
        let library = home.join("Library");
        folders.push(library.join("WebKit").join(identifier));
        folders.push(library.join("HTTPStorages").join(identifier));
        folders.push(library.join("Saved Application State").join(format!("{identifier}.savedState")));
    }
    folders.sort();
    folders.dedup();
    folders
}

/// Things outside those folders that Hindsight set up: launch at login and the shortcut.
pub fn undo_system_changes(app: &AppHandle) {
    let _ = app.autolaunch().disable();
    let _ = app.global_shortcut().unregister_all();
}

/// Runs at exit, if asked for.
pub fn remove_if_asked(app: &AppHandle) {
    if !app.state::<RemoveOnExit>().0.load(Ordering::SeqCst) {
        return;
    }
    undo_system_changes(app);
    for folder in data_folders(app) {
        let _ = std::fs::remove_dir_all(folder);
    }
}
