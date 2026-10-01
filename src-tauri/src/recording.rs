//! The recorder, as the application sees it: started at launch, its status readable by the
//! window, and what happens to it sent to the window as events.

use std::sync::Mutex;
use std::time::Duration;

use hindsight_core::capture::Role;
use hindsight_core::recorder::{Event, LossReason, Recorder, Settings};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

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

pub const DEFAULT_BUFFER_MINUTES: f64 = 15.0;

/// The menu's first line.
fn status_line(event: &Event) -> String {
    match event {
        Event::Recording { device, role: Role::Fallback } => format!("Recording from {device} (fallback)"),
        Event::Recording { device, .. } => format!("Recording from {device}"),
        Event::Lost { device, .. } => format!("Lost {device}; switching microphones"),
        Event::Waiting { .. } => "Not recording: no microphone is working".into(),
    }
}

pub fn start(app: &AppHandle) {
    let handle = app.clone();
    let settings = Settings { buffer_minutes: DEFAULT_BUFFER_MINUTES, ..Settings::default() };
    let recorder = Recorder::start(settings, move |event| {
        tray::set_status(&handle, status_line(&event));
        let _ = handle.emit("recorder", notice(event));
    });
    *app.state::<Recording>().0.lock().expect("recording") = Some(recorder);
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
                let buffered = match recording.0.lock().expect("recording").as_ref() {
                    Some(recorder) => recorder.buffered(),
                    None => return, // quitting
                };
                let text = holding(buffered.as_secs_f64(), DEFAULT_BUFFER_MINUTES * 60.0);
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
pub fn recorder_status(recording: State<'_, Recording>) -> Status {
    let recording = recording.0.lock().expect("recording");
    let Some(recorder) = recording.as_ref() else {
        return Status { microphone: None, role: None, buffered_seconds: 0.0, buffer_seconds: 0.0 };
    };
    let microphone = recorder.microphone();
    Status {
        role: microphone.as_ref().map(|(_, role)| role_name(*role)),
        microphone: microphone.map(|(name, _)| name),
        buffered_seconds: recorder.buffered().as_secs_f64(),
        buffer_seconds: Duration::from_secs_f64(DEFAULT_BUFFER_MINUTES * 60.0).as_secs_f64(),
    }
}
