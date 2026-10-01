//! The recorder, as the application sees it: started at launch, its status readable by the
//! window, and what happens to it sent to the window as events.

use std::sync::Mutex;
use std::time::Duration;

use hindsight_core::capture::Role;
use hindsight_core::recorder::{Event, LossReason, Recorder, Settings};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

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

pub fn start(app: &AppHandle) {
    let handle = app.clone();
    let settings = Settings { buffer_minutes: DEFAULT_BUFFER_MINUTES, ..Settings::default() };
    let recorder = Recorder::start(settings, move |event| {
        let _ = handle.emit("recorder", notice(event));
    });
    *app.state::<Recording>().0.lock().expect("recording") = Some(recorder);
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
