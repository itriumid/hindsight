//! Hindsight's application: keeps the process's memory out of crash dumps, records from the
//! moment the first-run screen is done, lives in the menu bar, and saves clips on request.

mod clips;
mod commands;
mod data;
mod hotkeys;
mod recording;
mod saving;
mod settings;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use settings::SettingsStore;

/// Launch at login starts Hindsight with this, so it starts quietly in the menu bar.
const BACKGROUND: &str = "--background";

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
        // First, so a second launch hands over to the running Hindsight before doing anything:
        // two copies would mean two recorders.
        .plugin(tauri_plugin_single_instance::init(|app, _arguments, _directory| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![BACKGROUND])))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(hotkeys::handle)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(recording::Recording(Mutex::new(None)))
        .manage(saving::LastClip::default())
        .manage(clips::Playback::default())
        .manage(data::RemoveOnExit::default())
        .setup(|app| {
            let handle = app.handle();
            app.manage(SettingsStore::load(handle)?);
            let settings = app.state::<SettingsStore>().get();

            #[cfg(target_os = "macos")]
            handle.set_dock_visibility(settings.show_in_dock)?;
            tray::create(handle)?;
            tray::set_visible(handle, settings.show_in_menu_bar);
            // A saved shortcut another app took since won't register; settings will say so when
            // someone tries to set it again, and the menu still saves.
            let _ = hotkeys::apply(handle, settings.save_hotkey.as_deref());

            if settings.welcomed {
                recording::start(handle);
            } else {
                tray::set_status(handle, "Not recording yet: finish setting up".into());
            }
            recording::keep_menu_current(handle);

            let quietly = std::env::args().any(|argument| argument == BACKGROUND);
            if !settings.welcomed || !quietly {
                tray::show_main_window(handle);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            recording::recorder_status,
            clips_folder,
            choose_clips_folder,
            reveal_clips_folder,
            save_last,
            commands::get_settings,
            commands::list_microphones,
            commands::set_microphones,
            commands::set_buffer_minutes,
            commands::set_save_hotkey,
            commands::set_presence,
            commands::set_launch_at_login,
            commands::finish_welcome,
            commands::pick_folder,
            commands::remove_all_data,
            clips::list_clips,
            clips::play_clip,
            clips::toggle_playback,
            clips::seek_playback,
            clips::stop_playback,
            clips::playback_state,
            clips::reveal_clip,
            clips::delete_clip,
            clips::export_clip
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
            recording::stop(app);
            app.state::<clips::Playback>().0.lock().expect("playback").take();
            data::remove_if_asked(app);
        }
    });
}
