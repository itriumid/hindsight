//! Hindsight's settings, kept as one small JSON file in its own data folder, written
//! atomically so a crash can never leave it half-written.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// False until the first-run screen is finished; nothing records before that.
    pub welcomed: bool,
    /// Where clips are saved; asked on the first save until it's chosen.
    pub clips_folder: Option<PathBuf>,
    /// The chosen microphone by name; `None` follows the system default.
    pub microphone: Option<String>,
    /// Used while the chosen microphone is away; `None` means the system default.
    pub fallback_microphone: Option<String>,
    /// How far back Hindsight can go: 15 to 180 minutes.
    pub buffer_minutes: u32,
    /// A global shortcut that saves; none by default.
    pub save_hotkey: Option<String>,
    /// How much the shortcut saves.
    pub save_hotkey_minutes: u32,
    /// macOS only: a Dock icon as well as the menu bar.
    pub show_in_dock: bool,
    pub show_in_menu_bar: bool,
    pub launch_at_login: bool,
    /// The copy of Hindsight that launch at login starts, so a copy that moved or was updated
    /// can tell it's no longer the one and point it at itself.
    pub launch_at_login_target: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            welcomed: false,
            clips_folder: None,
            microphone: None,
            fallback_microphone: None,
            buffer_minutes: 15,
            save_hotkey: None,
            save_hotkey_minutes: 15,
            show_in_dock: false,
            show_in_menu_bar: true,
            launch_at_login: false,
            launch_at_login_target: None,
        }
    }
}

pub const SHORTEST_BUFFER_MINUTES: u32 = 15;
pub const LONGEST_BUFFER_MINUTES: u32 = 180;

/// Can this window still be reached with these settings? On macOS, without a menu bar icon the
/// Dock icon is the only way back to the window, so they can't both be off. Elsewhere, opening
/// Hindsight again shows the running window (it never runs twice), so the tray icon can go.
pub fn reachable(show_in_dock: bool, show_in_menu_bar: bool) -> bool {
    show_in_menu_bar || show_in_dock || !cfg!(target_os = "macos")
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load(app: &AppHandle) -> Result<SettingsStore, String> {
        let folder = app.path().app_config_dir().map_err(|error| error.to_string())?;
        let path = folder.join("settings.json");
        // A missing or unreadable file means defaults, never a failed start.
        let current = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(SettingsStore { path, current: Mutex::new(current) })
    }

    pub fn get(&self) -> Settings {
        self.current.lock().expect("settings").clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        let mut current = self.current.lock().expect("settings");
        let mut next = current.clone();
        change(&mut next);
        write_atomically(&self.path, &serde_json::to_vec_pretty(&next).map_err(|error| error.to_string())?)?;
        *current = next.clone();
        Ok(next)
    }
}

fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let folder = path.parent().ok_or("settings path has no folder")?;
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        error.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_safe_ones() {
        let settings = Settings::default();
        assert!(!settings.welcomed, "nothing records before the first-run screen");
        assert!(!settings.launch_at_login);
        assert!(settings.save_hotkey.is_none());
        assert!(settings.show_in_menu_bar);
        assert_eq!(settings.buffer_minutes, 15);
    }

    #[test]
    fn an_old_file_missing_new_settings_still_loads() {
        let loaded: Settings = serde_json::from_str(r#"{"clipsFolder":"/tmp/clips"}"#).unwrap();
        assert_eq!(loaded.clips_folder, Some(PathBuf::from("/tmp/clips")));
        assert_eq!(loaded.buffer_minutes, 15);
        assert!(loaded.show_in_menu_bar);
    }

    #[test]
    fn hindsight_can_always_be_reached() {
        assert!(reachable(true, true));
        assert!(reachable(false, true));
        assert!(reachable(true, false));
        assert_eq!(reachable(false, false), !cfg!(target_os = "macos"));
    }
}
