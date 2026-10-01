//! Hindsight's settings, kept as one small JSON file in its own data folder, written
//! atomically so a crash can never leave it half-written.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where clips are saved; asked on the first save until it's chosen.
    pub clips_folder: Option<PathBuf>,
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
