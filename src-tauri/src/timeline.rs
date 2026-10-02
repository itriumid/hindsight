//! The timeline, as the window sees it: "Choose…" freezes everything the buffer holds, the
//! loudness bars stream to the window newest first, and the person listens, picks a start and an
//! end, and saves.
//!
//! Only one timeline is open at a time, and it closes, wiping its frozen copy, when the window
//! closes or recording restarts. Left open, it would keep audio the buffer has long since let go.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hindsight_core::player::Player;
use hindsight_core::timeline::Timeline;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::clips::Playback;
use crate::recording::Recording;
use crate::saving;

/// How many loudness bars the window gets: more than it has pixels for at most sizes.
const BARS: usize = 600;

/// How often bars are sent while they're being worked out.
const SEND_EVERY: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct OpenTimeline {
    current: Mutex<Option<Open>>,
    /// Counts openings, so bars still arriving from a closed timeline are told apart and dropped.
    generation: AtomicU64,
}

struct Open {
    timeline: Arc<Timeline>,
    /// When it was frozen: its end.
    frozen_at: Instant,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineView {
    generation: u64,
    seconds: f64,
    /// When it ends, in milliseconds since 1970, for clock times in the window.
    ends_at: u64,
    bars: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Levels {
    generation: u64,
    /// (bar, decibels), 0 being the oldest bar.
    bars: Vec<(usize, f32)>,
}

/// Freezes everything the buffer holds, replacing a timeline already open, and starts working
/// out its loudness bars, sent as "timeline-levels" events.
#[tauri::command]
pub fn open_timeline(app: AppHandle, open: State<'_, OpenTimeline>) -> Result<TimelineView, String> {
    close(&app);
    let timeline = {
        let recording = app.state::<Recording>();
        let recording = recording.0.lock().expect("recording");
        recording.as_ref().map(|recorder| recorder.freeze()).ok_or("Hindsight isn't recording.")?
    };
    if timeline.is_empty() {
        return Err("Nothing's been recorded yet.".into());
    }
    let timeline = Arc::new(timeline);
    let generation = open.generation.fetch_add(1, Ordering::SeqCst) + 1;
    *open.current.lock().expect("timeline") = Some(Open { timeline: Arc::clone(&timeline), frozen_at: Instant::now() });
    let view = TimelineView {
        generation,
        seconds: timeline.duration().as_secs_f64(),
        ends_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64),
        bars: BARS.min((timeline.duration().as_secs_f64() / 0.02) as usize),
    };

    let handle = app.clone();
    std::thread::spawn(move || {
        let open = handle.state::<OpenTimeline>();
        let mut batch = Vec::new();
        let mut sent = Instant::now();
        let _ = timeline.levels(BARS, |bar, level| {
            if open.generation.load(Ordering::SeqCst) != generation {
                return false; // closed, or replaced by a newer one
            }
            batch.push((bar, level));
            if sent.elapsed() >= SEND_EVERY {
                let _ = handle.emit("timeline-levels", Levels { generation, bars: std::mem::take(&mut batch) });
                sent = Instant::now();
            }
            true
        });
        if !batch.is_empty() && open.generation.load(Ordering::SeqCst) == generation {
            let _ = handle.emit("timeline-levels", Levels { generation, bars: batch });
        }
    });
    Ok(view)
}

fn current(app: &AppHandle) -> Result<(Arc<Timeline>, Instant), String> {
    let open = app.state::<OpenTimeline>();
    let current = open.current.lock().expect("timeline");
    current
        .as_ref()
        .map(|open| (Arc::clone(&open.timeline), open.frozen_at))
        .ok_or_else(|| "The timeline has closed. Open it again.".into())
}

/// Plays the timeline from `from_seconds`, through the same player as clips, so only one thing
/// plays at a time; `playback_state` follows it.
#[tauri::command]
pub fn listen_timeline(app: AppHandle, from_seconds: f64) -> Result<(), String> {
    let (timeline, _) = current(&app)?;
    let player = Player::play_timeline(timeline, Duration::from_secs_f64(from_seconds.max(0.0)))?;
    *app.state::<Playback>().0.lock().expect("playback") = Some(player);
    Ok(())
}

/// Saves `start_seconds` to `end_seconds` as a clip, then closes the timeline.
#[tauri::command]
pub fn save_timeline(app: AppHandle, start_seconds: f64, end_seconds: f64) -> Result<(), String> {
    let (timeline, frozen_at) = current(&app)?;
    let start = Duration::from_secs_f64(start_seconds.max(0.0));
    let end = Duration::from_secs_f64(end_seconds.max(start_seconds).max(0.0)).min(timeline.duration());
    if end <= start {
        return Err("Choose a stretch to save first.".into());
    }
    // The clip is named for when it ends: that long before the timeline was frozen, plus the
    // time since.
    let ended_ago = timeline.duration() - end + frozen_at.elapsed();
    saving::save_range(&app, timeline, start, end, ended_ago);
    close(&app);
    Ok(())
}

#[tauri::command]
pub fn close_timeline(app: AppHandle) {
    close(&app);
}

/// Closes the timeline, if one is open: stops listening to it and wipes its frozen copy (once a
/// save in progress has finished with it).
pub fn close(app: &AppHandle) {
    let Some(open) = app.try_state::<OpenTimeline>() else {
        return;
    };
    open.generation.fetch_add(1, Ordering::SeqCst);
    let closed = open.current.lock().expect("timeline").take();
    if closed.is_some() {
        let playback = app.state::<Playback>();
        let mut playing = playback.0.lock().expect("playback");
        // The timeline's player is the one without a clip.
        if playing.as_ref().is_some_and(|player| player.path().is_none()) {
            playing.take();
        }
    }
}
