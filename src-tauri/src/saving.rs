//! Saving a clip: freeze the newest minutes first, so nothing said is lost while anything else
//! happens; then make sure there's a folder (asking on the very first save), write the clip
//! atomically, and say so.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use hindsight_core::naming::{LocalTime, clip_path};
use hindsight_core::recorder::Snapshot;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::recording::Recording;
use crate::settings::SettingsStore;
use crate::tray;

/// The clip saved most recently, for "Show the last clip in its folder".
#[derive(Default)]
pub struct LastClip(pub Mutex<Option<PathBuf>>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub path: PathBuf,
    pub file_name: String,
    pub seconds: f64,
}

/// Saves the last `minutes` without blocking the caller: the folder dialog and the write run on
/// their own thread.
pub fn save_last(app: &AppHandle, minutes: u32) {
    // Freeze now, on the caller's thread, so the clip ends at the moment of the request.
    let snapshot = {
        let recording = app.state::<Recording>();
        let recording = recording.0.lock().expect("recording");
        recording.as_ref().map(|recorder| recorder.snapshot(Duration::from_secs(u64::from(minutes) * 60)))
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(snapshot) = snapshot.filter(|snapshot| !snapshot.is_empty()) else {
            notify(&app, "Nothing to save yet", "Hindsight hasn't recorded anything yet.");
            return;
        };
        match save(&app, &snapshot) {
            Ok(Some(saved)) => {
                *app.state::<LastClip>().0.lock().expect("last clip") = Some(saved.path.clone());
                tray::enable_show_last(&app);
                notify(&app, &format!("Saved the last {}", describe(saved.seconds)), &saved.file_name);
                let _ = app.emit("saved", saved);
            }
            Ok(None) => {} // discarded on purpose
            Err(error) => notify(&app, "Couldn't save the clip", &error),
        }
        // The snapshot, decrypted audio, wipes itself here.
    });
}

fn save(app: &AppHandle, snapshot: &Snapshot) -> Result<Option<Saved>, String> {
    let Some(folder) = clips_folder_or_ask(app)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(&folder).map_err(|error| format!("couldn't create {}: {error}", folder.display()))?;
    // Anything a crash left half-written goes first.
    let _ = hindsight_core::clip::sweep_partials(&folder);
    let path = clip_path(&folder, LocalTime::now());
    hindsight_core::clip::write(&path, snapshot.packets.iter(), snapshot.input_sample_rate)
        .map_err(|error| format!("couldn't write {}: {error}", path.display()))?;
    let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(Saved { path, file_name, seconds: snapshot.duration().as_secs_f64() }))
}

/// The chosen clips folder; on the first save, asks for one. `None` means the person chose to
/// discard this clip instead.
fn clips_folder_or_ask(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let settings = app.state::<SettingsStore>();
    if let Some(folder) = settings.get().clips_folder {
        return Ok(Some(folder));
    }
    loop {
        if let Some(folder) = pick_folder(app, "Where should Hindsight save clips?") {
            settings.update(|settings| settings.clips_folder = Some(folder.clone()))?;
            return Ok(Some(folder));
        }
        let discard = app
            .dialog()
            .message("This clip is held in memory until you choose a folder. Discard it?")
            .title("Discard this clip?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom("Discard".into(), "Choose a folder".into()))
            .blocking_show();
        if discard {
            return Ok(None);
        }
    }
}

pub fn pick_folder(app: &AppHandle, title: &str) -> Option<PathBuf> {
    let mut dialog = app.dialog().file().set_title(title);
    if let Ok(documents) = app.path().document_dir() {
        dialog = dialog.set_directory(documents);
    }
    dialog.blocking_pick_folder().and_then(|path| path.into_path().ok())
}

pub fn reveal(app: &AppHandle, path: &Path) {
    let _ = app.opener().reveal_item_in_dir(path);
}

pub fn reveal_last(app: &AppHandle) {
    let last = app.state::<LastClip>().0.lock().expect("last clip").clone();
    if let Some(path) = last.filter(|path| path.exists()) {
        reveal(app, &path);
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// "15 minutes", "1 minute", "40 seconds"
pub fn describe(seconds: f64) -> String {
    let whole = seconds.round() as u64;
    match whole {
        0..=59 => format!("{whole} second{}", if whole == 1 { "" } else { "s" }),
        _ => {
            let minutes = (whole + 30) / 60;
            format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn describes_lengths_in_plain_words() {
        assert_eq!(describe(1.0), "1 second");
        assert_eq!(describe(40.2), "40 seconds");
        assert_eq!(describe(60.0), "1 minute");
        assert_eq!(describe(299.6), "5 minutes");
        assert_eq!(describe(900.0), "15 minutes");
    }
}
