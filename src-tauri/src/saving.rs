//! Saving a clip: freeze the newest minutes first, so nothing said is lost while anything else
//! happens; then make sure there's a folder (asking on the very first save), write the clip
//! atomically, and say so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use hindsight_core::naming::{LocalTime, clip_path};
use hindsight_core::timeline::Timeline;
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
        let saved = save(&app, Duration::ZERO, snapshot.duration(), |path| {
            hindsight_core::clip::write(path, snapshot.packets.iter(), snapshot.input_sample_rate)
        });
        announce(&app, saved, |seconds| format!("Saved the last {}", describe(seconds)));
        // The snapshot, decrypted audio, wipes itself here.
    });
}

/// Saves `start` to `end` of the timeline, without blocking the caller. The clip is named for
/// the moment it ends: `ended_ago` before now.
pub fn save_range(app: &AppHandle, timeline: Arc<Timeline>, start: Duration, end: Duration, ended_ago: Duration) {
    let app = app.clone();
    std::thread::spawn(move || {
        let saved = save(&app, ended_ago, end.saturating_sub(start), |path| {
            timeline.save(path, start, end)
        });
        announce(&app, saved, |seconds| format!("Saved {}", describe(seconds)));
    });
}

/// Makes sure there's a folder (asking on the very first save), then has `write` write the clip
/// to a new path in it, named for the moment the clip ends.
fn save(
    app: &AppHandle,
    ended_ago: Duration,
    length: Duration,
    write: impl FnOnce(&Path) -> std::io::Result<usize>,
) -> Result<Option<Saved>, String> {
    let Some(folder) = clips_folder_or_ask(app)? else {
        return Ok(None);
    };
    let path = write_clip(&folder, ended_ago, write)?;
    let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(Saved { path, file_name, seconds: length.as_secs_f64() }))
}

/// Saves happen one at a time. Each starts by sweeping half-written clips out of the folder, which
/// would otherwise take one that another save is still writing (a shortcut pressed during a long
/// timeline save lost the timeline's clip), and two saves in the same second would both pick the
/// same free name. The audio is frozen before waiting, so waiting loses nothing.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Writes a clip into `folder` through `write`, at a new path named for when it ends: `ended_ago`
/// before now. Its modification time says the same, so the Clips list, which sorts by it, and
/// the file manager agree with the name, even for a stretch of the timeline that ended a while
/// before it was saved.
fn write_clip(folder: &Path, ended_ago: Duration, write: impl FnOnce(&Path) -> std::io::Result<usize>) -> Result<PathBuf, String> {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    std::fs::create_dir_all(folder).map_err(|error| format!("couldn't create {}: {error}", folder.display()))?;
    // Anything a crash left half-written goes first.
    let _ = hindsight_core::clip::sweep_partials(folder);
    let ended = SystemTime::now() - ended_ago;
    let path = clip_path(folder, LocalTime::before_now(ended_ago));
    write(&path).map_err(|error| format!("couldn't write {}: {error}", path.display()))?;
    // Only a date: if the system won't set it, the clip is still saved.
    let _ = std::fs::File::options().write(true).open(&path).and_then(|file| file.set_modified(ended));
    Ok(path)
}

/// Says how a save went, and tells the window about a new clip.
fn announce(app: &AppHandle, saved: Result<Option<Saved>, String>, title: impl FnOnce(f64) -> String) {
    match saved {
        Ok(Some(saved)) => {
            *app.state::<LastClip>().0.lock().expect("last clip") = Some(saved.path.clone());
            tray::enable_show_last(app);
            notify(app, &title(saved.seconds), &saved.file_name);
            let _ = app.emit("saved", saved);
        }
        Ok(None) => {} // discarded on purpose
        Err(error) => notify(app, "Couldn't save the clip", &error),
    }
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
    use super::*;

    /// A save still writing when another starts, as when the shortcut is pressed during a long
    /// timeline save: both clips have to come out whole.
    #[test]
    fn overlapping_saves_both_keep_their_clips() {
        let folder = std::env::temp_dir().join(format!("hindsight-overlap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let packets = || (0..50u8).map(|value| vec![value; 40]);
        let slow = {
            let folder = folder.clone();
            std::thread::spawn(move || {
                write_clip(&folder, Duration::ZERO, |path| {
                    // Each packet takes a while, so the clip is half-written for most of a second.
                    let slowly = packets().inspect(|_| std::thread::sleep(Duration::from_millis(15)));
                    hindsight_core::clip::write(path, slowly, 48_000)
                })
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        let quick = write_clip(&folder, Duration::ZERO, |path| hindsight_core::clip::write(path, packets(), 48_000));
        let slow = slow.join().unwrap();

        let (slow, quick) = (slow.expect("the slow save failed"), quick.expect("the quick save failed"));
        assert_ne!(slow, quick, "both saves picked the same name");
        for clip in [&slow, &quick] {
            assert_eq!(hindsight_core::clip::duration(clip).unwrap(), Duration::from_millis(920), "{}", clip.display());
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    /// A stretch of the timeline that ended 40 minutes ago is named and dated for then, not for
    /// when it was saved.
    #[test]
    fn a_clip_is_dated_for_when_it_ends() {
        let folder = std::env::temp_dir().join(format!("hindsight-dated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let ago = Duration::from_secs(40 * 60);
        let packets = (0..10u8).map(|value| vec![value; 40]);
        let path = write_clip(&folder, ago, |path| hindsight_core::clip::write(path, packets, 48_000)).unwrap();

        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let off = SystemTime::now().duration_since(modified).unwrap().abs_diff(ago);
        assert!(off < Duration::from_secs(5), "dated {off:?} away from when it ended");
        let named = hindsight_core::naming::clip_stem(LocalTime::before_now(ago));
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        // The two readings of the clock can straddle a second.
        assert_eq!(name[..name.len() - 2], named[..named.len() - 2], "{name} isn't named for 40 minutes ago");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn describes_lengths_in_plain_words() {
        assert_eq!(describe(1.0), "1 second");
        assert_eq!(describe(40.2), "40 seconds");
        assert_eq!(describe(60.0), "1 minute");
        assert_eq!(describe(299.6), "5 minutes");
        assert_eq!(describe(900.0), "15 minutes");
    }
}
