//! Hindsight's application: keeps the process's memory out of crash dumps, records from launch,
//! lives in the menu bar, and saves clips on request.

mod recording;
mod saving;
mod settings;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent};

use settings::SettingsStore;

#[tauri::command]
fn clips_folder(settings: State<'_, SettingsStore>) -> Option<PathBuf> {
    settings.get().clips_folder
}

/// Runs on Tauri's thread pool, so the folder dialog can block without freezing anything.
#[tauri::command]
async fn choose_clips_folder(app: AppHandle) -> Result<Option<PathBuf>, String> {
    let Some(folder) = saving::pick_folder(&app, "Where should Hindsight save clips?") else {
        return Ok(app.state::<SettingsStore>().get().clips_folder);
    };
    let settings = app.state::<SettingsStore>().update(|settings| settings.clips_folder = Some(folder.clone()))?;
    Ok(settings.clips_folder)
}

#[tauri::command]
fn reveal_clips_folder(app: AppHandle, settings: State<'_, SettingsStore>) {
    if let Some(folder) = settings.get().clips_folder.filter(|folder| folder.exists()) {
        saving::reveal(&app, &folder);
    }
}

#[tauri::command]
fn save_last(app: AppHandle, minutes: u32) {
    saving::save_last(&app, minutes);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // First thing: anything the process holds later (the recording, its key) must never end
    // up in a crash dump.
    hindsight_core::privacy::keep_memory_out_of_crash_dumps();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(recording::Recording(Mutex::new(None)))
        .manage(saving::LastClip::default())
        .setup(|app| {
            let handle = app.handle();
            app.manage(SettingsStore::load(handle)?);
            // Menu bar only by default: an always-on recorder doesn't need a Dock icon.
            #[cfg(target_os = "macos")]
            handle.set_dock_visibility(false)?;
            tray::create(handle)?;
            recording::start(handle);
            recording::keep_menu_current(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            recording::recorder_status,
            clips_folder,
            choose_clips_folder,
            reveal_clips_folder,
            save_last
        ])
        .on_window_event(|window, event| {
            // Closing the window keeps Hindsight recording in the menu bar; Quit is in the menu.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Hindsight");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Stopping the recorder joins its threads: the stream closes and the buffer and its
            // key are wiped before the process ends.
            if let Some(recording) = app.try_state::<recording::Recording>() {
                recording.0.lock().expect("recording").take();
            }
        }
    });
}
