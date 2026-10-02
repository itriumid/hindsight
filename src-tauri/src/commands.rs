//! What the window can ask for: settings, the first-run screen, and removing everything.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::data::RemoveOnExit;
use crate::settings::{LONGEST_BUFFER_MINUTES, SHORTEST_BUFFER_MINUTES, Settings, SettingsStore, reachable};
use crate::{hotkeys, launch_at_login, recording, saving, tray};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    /// The Dock exists only on macOS.
    has_dock: bool,
}

fn view(app: &AppHandle) -> SettingsView {
    let mut settings = app.state::<SettingsStore>().get();
    // What the system actually has, in case it changed behind Hindsight's back.
    settings.launch_at_login = app.autolaunch().is_enabled().unwrap_or(settings.launch_at_login);
    SettingsView { settings, has_dock: cfg!(target_os = "macos") }
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    view(&app)
}

#[tauri::command]
pub fn list_microphones() -> Vec<String> {
    hindsight_core::capture::input_device_names()
}

#[tauri::command]
pub fn set_microphones(app: AppHandle, microphone: Option<String>, fallback: Option<String>) -> Result<SettingsView, String> {
    app.state::<SettingsStore>().update(|settings| {
        settings.microphone = microphone;
        settings.fallback_microphone = fallback;
    })?;
    recording::set_microphones(&app);
    Ok(view(&app))
}

/// Changing the buffer's length starts it again: the old buffer is wiped, not resized.
#[tauri::command]
pub fn set_buffer_minutes(app: AppHandle, minutes: u32) -> Result<SettingsView, String> {
    let minutes = minutes.clamp(SHORTEST_BUFFER_MINUTES, LONGEST_BUFFER_MINUTES);
    let settings = app.state::<SettingsStore>().update(|settings| settings.buffer_minutes = minutes)?;
    if settings.welcomed {
        recording::start(&app);
    }
    Ok(view(&app))
}

/// Registers the shortcut first and only saves it once the system has accepted it.
#[tauri::command]
pub fn set_save_hotkey(app: AppHandle, hotkey: Option<String>, minutes: u32) -> Result<SettingsView, String> {
    let previous = app.state::<SettingsStore>().get().save_hotkey;
    if let Err(error) = hotkeys::apply(&app, hotkey.as_deref()) {
        let _ = hotkeys::apply(&app, previous.as_deref());
        return Err(error);
    }
    app.state::<SettingsStore>().update(|settings| {
        settings.save_hotkey = hotkey;
        settings.save_hotkey_minutes = minutes.max(1);
    })?;
    Ok(view(&app))
}

/// Where Hindsight shows up, both at once: switching from "menu bar only" to "Dock only" one
/// setting at a time would pass through "neither".
#[tauri::command]
pub fn set_presence(app: AppHandle, menu_bar: bool, dock: bool) -> Result<SettingsView, String> {
    // Without a Dock, the Dock setting doesn't apply.
    let dock = dock && cfg!(target_os = "macos");
    if !reachable(dock, menu_bar) {
        return Err("Hindsight needs the menu bar or the Dock, so you can always open it.".into());
    }
    #[cfg(target_os = "macos")]
    app.set_dock_visibility(dock).map_err(|error| error.to_string())?;
    tray::set_visible(&app, menu_bar);
    app.state::<SettingsStore>().update(|settings| {
        settings.show_in_menu_bar = menu_bar;
        settings.show_in_dock = dock;
    })?;
    Ok(view(&app))
}

#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, on: bool) -> Result<SettingsView, String> {
    launch_at_login::set(&app, on)?;
    Ok(view(&app))
}

/// The first-run screen's "Start recording": nothing records before this.
#[tauri::command]
pub fn finish_welcome(app: AppHandle, launch_at_login: bool, clips_folder: Option<PathBuf>) -> Result<SettingsView, String> {
    if launch_at_login {
        launch_at_login::set(&app, true)?;
    }
    app.state::<SettingsStore>().update(|settings| {
        settings.welcomed = true;
        if clips_folder.is_some() {
            settings.clips_folder = clips_folder;
        }
    })?;
    recording::start(&app);
    Ok(view(&app))
}

/// A folder picker for the first-run screen, which doesn't save the choice yet.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Option<PathBuf> {
    saving::pick_folder(&app, "Where should Hindsight save clips?")
}

/// Asks first; on yes, stops recording, quits, and removes everything as the app exits.
#[tauri::command]
pub async fn remove_all_data(app: AppHandle, remove: State<'_, RemoveOnExit>) -> Result<(), String> {
    let confirmed = app
        .dialog()
        .message(
            "This removes Hindsight's settings and everything it stores for itself, turns off launch at login and the save shortcut, and quits. Clips you saved stay where they are.",
        )
        .title("Remove all Hindsight data?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Remove and quit".into(), "Cancel".into()))
        .blocking_show();
    if confirmed {
        remove.0.store(true, Ordering::SeqCst);
        app.exit(0);
    }
    Ok(())
}
