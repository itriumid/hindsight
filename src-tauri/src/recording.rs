//! The recorder, as the application sees it: started at launch, its status readable by the
//! window, and what happens to it sent to the window as events.

use std::sync::Mutex;
use std::time::Duration;

use hindsight_core::capture::Role;
use hindsight_core::recorder::{Event, LossReason, Recorder, Settings};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings::SettingsStore;
use crate::tray;

pub struct Recording(pub Mutex<Option<Recorder>>);

/// What the window shows. Plain data, so the core doesn't need to know about serde.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The microphone recording now, if any.
    pub microphone: Option<String>,
    /// "primary", "fallback" or "systemDefault".
    pub role: Option<&'static str>,
    pub buffered_seconds: f64,
    pub buffer_seconds: f64,
}

/// One thing that happened, for the window's status line.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Notice {
    Recording { microphone: String, role: &'static str },
    Lost { microphone: String, role: &'static str, reason: String },
    Waiting { retry_in_seconds: u64 },
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Primary => "primary",
        Role::Fallback => "fallback",
        Role::SystemDefault => "systemDefault",
    }
}

fn notice(event: Event) -> Notice {
    match event {
        Event::Recording { device, role } => Notice::Recording { microphone: device, role: role_name(role) },
        Event::Lost { device, role, reason } => Notice::Lost {
            microphone: device,
            role: role_name(role),
            reason: match reason {
                LossReason::Reported(message) => message,
                LossReason::NoAudio => "it stopped sending audio".into(),
                LossReason::OnlySilence => "it only sent silence".into(),
            },
        },
        Event::Waiting { retry_in } => Notice::Waiting { retry_in_seconds: retry_in.as_secs() },
    }
}

/// The menu's first line.
fn status_line(event: &Event) -> String {
    match event {
        Event::Recording { device, role: Role::Fallback } => format!("Recording from {device} (fallback)"),
        Event::Recording { device, .. } => format!("Recording from {device}"),
        Event::Lost { device, .. } => format!("Lost {device}; switching microphones"),
        Event::Waiting { .. } => "Not recording: no microphone is working".into(),
    }
}

/// Debug builds only: `HINDSIGHT_FILL=<clip.opus>` starts the buffer with that clip, as if it
/// had just been recorded, to try the timeline with hours held without waiting hours.
/// `cargo run -p hindsight-spike -- demo 72` makes one. Release builds never read it.
fn start_with() -> Option<std::path::PathBuf> {
    if cfg!(debug_assertions) { std::env::var_os("HINDSIGHT_FILL").map(Into::into) } else { None }
}

/// Starts recording with the saved settings, replacing a recorder that's already running. An
/// open timeline closes: the buffer it was frozen from is being replaced.
pub fn start(app: &AppHandle) {
    crate::timeline::close(app);
    let _ = app.emit("timeline-closed", ());
    let saved = app.state::<SettingsStore>().get();
    let settings = Settings {
        primary: saved.microphone,
        fallback: saved.fallback_microphone,
        buffer_minutes: f64::from(saved.buffer_minutes),
        start_with: start_with(),
        ..Settings::default()
    };
    let handle = app.clone();
    // The old recorder, if any, stops first: it wipes its buffer and key, and frees the
    // microphone for the new one.
    stop(app);
    let recorder = Recorder::start(settings, move |event| {
        tray::set_status(&handle, status_line(&event));
        let _ = handle.emit("recorder", notice(event));
    });
    *app.state::<Recording>().0.lock().expect("recording") = Some(recorder);
}

pub fn stop(app: &AppHandle) {
    let old = app.state::<Recording>().0.lock().expect("recording").take();
    drop(old); // joins the threads outside the lock
}

pub fn set_microphones(app: &AppHandle) {
    let saved = app.state::<SettingsStore>().get();
    if let Some(recorder) = app.state::<Recording>().0.lock().expect("recording").as_ref() {
        recorder.set_microphones(saved.microphone, saved.fallback_microphone);
    }
}

/// Keeps the menu's "Holding the last …" line current, once a second, changing it only when
/// the text changes.
pub fn keep_menu_current(app: &AppHandle) {
    let handle = app.clone();
    std::thread::Builder::new()
        .name("hindsight-menu".into())
        .spawn(move || {
            let mut shown = String::new();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(recording) = handle.try_state::<Recording>() else { continue };
                let buffered = recording.0.lock().expect("recording").as_ref().map(|recorder| recorder.buffered());
                let buffer_seconds = f64::from(handle.state::<SettingsStore>().get().buffer_minutes) * 60.0;
                let text = match buffered {
                    Some(buffered) => holding(buffered.as_secs_f64(), buffer_seconds),
                    None => "Not recording".into(),
                };
                if text != shown {
                    tray::set_buffered(&handle, text.clone());
                    shown = text;
                }
            }
        })
        .expect("couldn't start the menu thread");
}

/// "Holding the last 4 min 12 s"; "Holding the last 15 minutes" once the buffer is full.
pub fn holding(buffered_seconds: f64, buffer_seconds: f64) -> String {
    let whole = buffered_seconds.floor() as u64;
    if whole == 0 {
        return "Holding nothing yet".into();
    }
    if buffered_seconds >= buffer_seconds - 0.5 {
        let minutes = (buffer_seconds / 60.0).round() as u64;
        return if minutes >= 60 && minutes % 60 == 0 {
            let hours = minutes / 60;
            format!("Holding the last {hours} hour{}", if hours == 1 { "" } else { "s" })
        } else {
            format!("Holding the last {minutes} minutes")
        };
    }
    let (hours, minutes, seconds) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    match (hours, minutes) {
        (0, 0) => format!("Holding the last {seconds} s"),
        (0, _) => format!("Holding the last {minutes} min {seconds} s"),
        _ => format!("Holding the last {hours} h {minutes} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::holding;

    #[test]
    fn describes_the_buffer_like_the_window() {
        assert_eq!(holding(0.4, 900.0), "Holding nothing yet");
        assert_eq!(holding(42.7, 900.0), "Holding the last 42 s");
        assert_eq!(holding(252.0, 900.0), "Holding the last 4 min 12 s");
        assert_eq!(holding(900.0, 900.0), "Holding the last 15 minutes");
        assert_eq!(holding(5400.0, 10800.0), "Holding the last 1 h 30 min");
        assert_eq!(holding(10800.0, 10800.0), "Holding the last 3 hours");
    }
}

#[tauri::command]
pub fn recorder_status(recording: State<'_, Recording>, settings: State<'_, SettingsStore>) -> Status {
    let buffer_seconds = f64::from(settings.get().buffer_minutes) * 60.0;
    let recording = recording.0.lock().expect("recording");
    let Some(recorder) = recording.as_ref() else {
        return Status { microphone: None, role: None, buffered_seconds: 0.0, buffer_seconds };
    };
    let microphone = recorder.microphone();
    Status {
        role: microphone.as_ref().map(|(_, role)| role_name(*role)),
        microphone: microphone.map(|(name, _)| name),
        buffered_seconds: recorder.buffered().as_secs_f64(),
        buffer_seconds,
    }
}
