//! The save shortcut: a global shortcut, so it works while another app has focus. There's none
//! by default; settings can set one.

use std::str::FromStr;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::saving;
use crate::settings::SettingsStore;

/// Registers `hotkey` in place of whatever was registered; `None` clears it.
pub fn apply(app: &AppHandle, hotkey: Option<&str>) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    shortcuts.unregister_all().map_err(|error| format!("couldn't clear the shortcut: {error}"))?;
    let Some(hotkey) = hotkey else { return Ok(()) };
    let shortcut = Shortcut::from_str(hotkey).map_err(|error| format!("that isn't a shortcut Hindsight understands: {error}"))?;
    shortcuts
        .register(shortcut)
        .map_err(|_| "another app is already using that shortcut; try a different one".to_string())
}

/// The plugin's handler: the shortcut saves the configured number of minutes.
pub fn handle(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state != ShortcutState::Pressed {
        return;
    }
    let minutes = app.state::<SettingsStore>().get().save_hotkey_minutes;
    saving::save_last(app, minutes);
}
