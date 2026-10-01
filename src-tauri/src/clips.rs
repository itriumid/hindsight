//! The clip list: what's in the clips folder, playing a clip, showing it in its folder, exporting
//! it as WAV and deleting it.
//!
//! The window names clips by path, so every command first checks the path really is a clip in
//! the clips folder. A bug or anything injected into the window can't use these commands to read
//! or delete other files.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use hindsight_core::clip::{PARTIAL_SUFFIX, duration, export_wav as write_wav};
use hindsight_core::player::Player;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;

use crate::saving;
use crate::settings::SettingsStore;

/// The clip playing now, if any.
#[derive(Default)]
pub struct Playback(pub Mutex<Option<Player>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub path: PathBuf,
    pub name: String,
    pub seconds: f64,
    pub bytes: u64,
    /// Seconds since 1970, for sorting and showing when it was saved.
    pub saved_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub path: Option<PathBuf>,
    pub position_seconds: f64,
    pub length_seconds: f64,
    pub playing: bool,
}

/// `path` if it's a clip in the clips folder, else an error.
fn checked(app: &AppHandle, path: &Path) -> Result<PathBuf, String> {
    let folder = app.state::<SettingsStore>().get().clips_folder.ok_or("no clips folder chosen")?;
    clip_in(&folder, path)
}

/// `path`, resolved, if it's a visible `.opus` file directly inside `folder`. Resolving first
/// means `..` and symbolic links can't point anywhere else.
fn clip_in(folder: &Path, path: &Path) -> Result<PathBuf, String> {
    let folder = folder.canonicalize().map_err(|error| error.to_string())?;
    let path = path.canonicalize().map_err(|_| "that clip doesn't exist any more".to_string())?;
    let is_clip = path.is_file()
        && path.parent() == Some(folder.as_path())
        && path.extension().is_some_and(|extension| extension == "opus")
        && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| !name.starts_with('.'));
    if is_clip { Ok(path) } else { Err("that isn't a clip in your clips folder".into()) }
}

/// Every clip in the clips folder, newest first.
#[tauri::command]
pub fn list_clips(settings: State<'_, SettingsStore>) -> Vec<Clip> {
    let Some(folder) = settings.get().clips_folder else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&folder) else { return Vec::new() };
    let mut clips: Vec<Clip> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            if name.starts_with('.') || name.ends_with(PARTIAL_SUFFIX) || path.extension()? != "opus" {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(Clip {
                seconds: duration(&path).map(|length| length.as_secs_f64()).unwrap_or(0.0),
                bytes: metadata.len(),
                saved_at: metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs(),
                path,
                name,
            })
        })
        .collect();
    clips.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then_with(|| b.name.cmp(&a.name)));
    clips
}

#[tauri::command]
pub fn play_clip(app: AppHandle, playback: State<'_, Playback>, path: PathBuf) -> Result<(), String> {
    let path = checked(&app, &path)?;
    let mut current = playback.0.lock().expect("playback");
    // Stop the old one first, so only one clip ever plays.
    current.take();
    *current = Some(Player::play(&path)?);
    Ok(())
}

#[tauri::command]
pub fn toggle_playback(playback: State<'_, Playback>) {
    if let Some(player) = playback.0.lock().expect("playback").as_ref() {
        if player.is_playing() { player.pause() } else { player.resume() }
    }
}

#[tauri::command]
pub fn seek_playback(playback: State<'_, Playback>, seconds: f64) {
    if let Some(player) = playback.0.lock().expect("playback").as_ref() {
        player.seek(Duration::from_secs_f64(seconds.max(0.0)));
    }
}

#[tauri::command]
pub fn stop_playback(playback: State<'_, Playback>) {
    playback.0.lock().expect("playback").take();
}

#[tauri::command]
pub fn playback_state(playback: State<'_, Playback>) -> PlaybackState {
    match playback.0.lock().expect("playback").as_ref() {
        Some(player) => PlaybackState {
            path: Some(player.path().to_path_buf()),
            position_seconds: player.position().as_secs_f64(),
            length_seconds: player.length().as_secs_f64(),
            playing: player.is_playing(),
        },
        None => PlaybackState { path: None, position_seconds: 0.0, length_seconds: 0.0, playing: false },
    }
}

#[tauri::command]
pub fn reveal_clip(app: AppHandle, path: PathBuf) -> Result<(), String> {
    saving::reveal(&app, &checked(&app, &path)?);
    Ok(())
}

/// Asks first, then deletes the clip for good: a privacy tool shouldn't leave a copy in the bin.
#[tauri::command]
pub async fn delete_clip(app: AppHandle, path: PathBuf) -> Result<bool, String> {
    let path = checked(&app, &path)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let confirmed = app
        .dialog()
        .message(format!("\"{name}\" will be deleted for good. It won't go to the bin."))
        .title("Delete this clip?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Delete".into(), "Cancel".into()))
        .blocking_show();
    if !confirmed {
        return Ok(false);
    }
    {
        let playback = app.state::<Playback>();
        let mut current = playback.0.lock().expect("playback");
        if current.as_ref().is_some_and(|player| player.path() == path) {
            current.take();
        }
    }
    std::fs::remove_file(&path).map_err(|error| format!("couldn't delete it: {error}"))?;
    Ok(true)
}

/// Asks where to save a WAV copy, then writes it on its own thread and says when it's done.
#[tauri::command]
pub async fn export_clip(app: AppHandle, path: PathBuf) -> Result<(), String> {
    let path = checked(&app, &path)?;
    let suggested = path.with_extension("wav").file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dialog = app.dialog().file().set_title("Export as WAV").set_file_name(&suggested).add_filter("WAV audio", &["wav"]);
    if let Some(folder) = path.parent() {
        dialog = dialog.set_directory(folder);
    }
    let Some(destination) = dialog.blocking_save_file().and_then(|chosen| chosen.into_path().ok()) else {
        return Ok(());
    };
    std::thread::spawn(move || {
        let (title, body) = match write_wav(&path, &destination) {
            Ok(()) => ("Exported as WAV", destination.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()),
            Err(error) => ("Couldn't export the clip", error),
        };
        let _ = app.notification().builder().title(title).body(body).show();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::clip_in;
    use std::path::PathBuf;

    fn setup() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("hindsight-clip-check-{}", std::process::id()));
        let folder = root.join("clips");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        for name in ["a.opus", ".hidden.opus", "notes.txt", "sub/b.opus"] {
            std::fs::write(folder.join(name), b"").unwrap();
        }
        std::fs::create_dir_all(folder.join("folder.opus")).unwrap();
        std::fs::write(root.join("outside.opus"), b"").unwrap();
        (root, folder)
    }

    #[test]
    fn only_visible_clips_directly_in_the_folder_pass() {
        let (root, folder) = setup();
        assert!(clip_in(&folder, &folder.join("a.opus")).is_ok());
        for refused in [
            folder.join("notes.txt"),             // not a clip
            folder.join(".hidden.opus"),          // hidden, like a half-written save
            folder.join("sub/b.opus"),            // in a subfolder
            folder.join("folder.opus"),           // a folder named like a clip
            folder.join("../outside.opus"),       // climbing out
            folder.join("missing.opus"),          // doesn't exist
        ] {
            assert!(clip_in(&folder, &refused).is_err(), "{} should be refused", refused.display());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
