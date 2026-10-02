//! The always-on recorder: keeps a microphone open, encodes what it hears into the encrypted
//! ring buffer, and moves between microphones when one goes away.
//!
//! Two threads. The controller owns the microphone streams (cpal streams can't move between
//! threads) and decides which microphone to use; the encoder turns audio into packets and pushes
//! them into the ring. Saving takes a decrypted copy of the newest packets from the ring, so a
//! save never pauses recording.
//!
//! Which microphone, when to give up on one and how long to wait before trying again is decided
//! by `Switcher`, which has no threads or devices in it so it can be tested on its own.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::capture::{Audio, Heartbeat, Resampler, Role, choose_device, device_name, find_device, open_stream};
use crate::encoding::{DEFAULT_COMPLEXITY, FRAME, PACKET_BYTES, PACKET_SECONDS, encoder, ring_for};
use crate::ring::Ring;
use crate::timeline::Timeline;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Part of the chosen microphone's name; `None` follows the system default.
    pub primary: Option<String>,
    /// Used while the chosen microphone is away; `None` means the system default.
    pub fallback: Option<String>,
    pub buffer_minutes: f64,
    pub complexity: u8,
    /// How much audio the system hands over per wake-up; `None` lets it decide.
    pub buffer_frames: Option<u32>,
    /// A clip to start the buffer with, as if it had just been recorded, before the microphone's
    /// audio: for testing with hours held without waiting hours. The application only sets it in
    /// debug builds.
    pub start_with: Option<std::path::PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            primary: None,
            fallback: None,
            buffer_minutes: 15.0,
            complexity: DEFAULT_COMPLEXITY,
            buffer_frames: None,
            start_with: None,
        }
    }
}

/// What happened, for the application to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Recording { device: String, role: Role },
    Lost { device: String, role: Role, reason: LossReason },
    /// No microphone could be opened, or every one tried sent nothing; trying again after `retry_in`.
    Waiting { retry_in: Duration },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LossReason {
    /// The system said the device went away.
    Reported(String),
    /// No audio arrived at all for a while.
    NoAudio,
    /// Audio kept arriving, but only exact zeros: a live microphone always has some noise.
    OnlySilence,
}

/// A frozen copy of the newest audio, ready to be written as a clip.
pub struct Snapshot {
    pub packets: Vec<Vec<u8>>,
    pub input_sample_rate: u32,
}

impl Snapshot {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.packets.len() as f64 * PACKET_SECONDS)
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

impl Drop for Snapshot {
    /// A snapshot is decrypted audio; saved or discarded, it doesn't linger in freed memory.
    fn drop(&mut self) {
        for packet in &mut self.packets {
            for byte in packet.iter_mut() {
                unsafe { std::ptr::write_volatile(byte, 0) };
            }
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
}

enum Command {
    SetMicrophones { primary: Option<String>, fallback: Option<String> },
    Stop,
}

pub struct Recorder {
    commands: Sender<Command>,
    ring: Arc<Mutex<Ring>>,
    input_rate: Arc<AtomicU32>,
    current: Arc<Mutex<Option<(String, Role)>>>,
    controller: Option<JoinHandle<()>>,
    encoding: Option<JoinHandle<()>>,
}

impl Recorder {
    /// Starts recording straight away. `on_event` is called from the recorder's own thread.
    pub fn start(settings: Settings, on_event: impl Fn(Event) + Send + 'static) -> Recorder {
        let ring = Arc::new(Mutex::new(ring_for(settings.buffer_minutes)));
        let input_rate = Arc::new(AtomicU32::new(48_000));
        let current = Arc::new(Mutex::new(None));
        let (audio_sender, audio_receiver) = mpsc::channel::<Audio>();
        let (commands, command_receiver) = mpsc::channel::<Command>();

        let encoding = {
            let ring = Arc::clone(&ring);
            let input_rate = Arc::clone(&input_rate);
            let complexity = settings.complexity;
            let start_with = settings.start_with.clone();
            std::thread::Builder::new()
                .name("hindsight-encoder".into())
                .spawn(move || {
                    // Before the first of the microphone's audio, which waits in the channel.
                    if let Some(path) = start_with
                        && let Err(error) = start_buffer(&mut ring.lock().expect("ring"), &path)
                    {
                        eprintln!("couldn't start the buffer with {}: {error}", path.display());
                    }
                    encode(audio_receiver, ring, input_rate, complexity)
                })
                .expect("couldn't start the encoder thread")
        };
        let controller = {
            let current = Arc::clone(&current);
            std::thread::Builder::new()
                .name("hindsight-microphones".into())
                .spawn(move || control(settings, audio_sender, command_receiver, current, on_event))
                .expect("couldn't start the microphone thread")
        };
        Recorder { commands, ring, input_rate, current, controller: Some(controller), encoding: Some(encoding) }
    }

    /// The newest `duration` of audio, decrypted, without pausing recording.
    pub fn snapshot(&self, duration: Duration) -> Snapshot {
        let count = (duration.as_secs_f64() / PACKET_SECONDS).round() as usize;
        let ring = self.ring.lock().expect("ring");
        Snapshot { packets: ring.newest(count).collect(), input_sample_rate: self.input_rate.load(Ordering::Relaxed) }
    }

    /// Everything the buffer holds, frozen and still encrypted, for the timeline. Recording
    /// carries on into the buffer meanwhile.
    pub fn freeze(&self) -> Timeline {
        let ring = self.ring.lock().expect("ring");
        Timeline::new(ring.freeze(ring.len()), self.input_rate.load(Ordering::Relaxed))
    }

    /// How much audio the buffer holds right now.
    pub fn buffered(&self) -> Duration {
        Duration::from_secs_f64(self.ring.lock().expect("ring").len() as f64 * PACKET_SECONDS)
    }

    /// The microphone recording right now, if any.
    pub fn microphone(&self) -> Option<(String, Role)> {
        self.current.lock().expect("current").clone()
    }

    /// Wipes the buffer and switches to a new encryption key; recording carries on.
    pub fn forget(&self) {
        self.ring.lock().expect("ring").clear();
    }

    pub fn set_microphones(&self, primary: Option<String>, fallback: Option<String>) {
        let _ = self.commands.send(Command::SetMicrophones { primary, fallback });
    }

    /// Read-only access to the ring, for diagnostics.
    pub fn with_ring<T>(&self, inspect: impl FnOnce(&Ring) -> T) -> T {
        inspect(&self.ring.lock().expect("ring"))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        // The controller drops its stream and its audio sender, which ends the encoder.
        if let Some(controller) = self.controller.take() {
            let _ = controller.join();
        }
        if let Some(encoding) = self.encoding.take() {
            let _ = encoding.join();
        }
    }
}

/// Pushes a clip's packets into the buffer, oldest first; returns how many. Only clips made
/// with Hindsight's encoder fit, since every slot holds one of its fixed-size packets.
fn start_buffer(ring: &mut Ring, path: &std::path::Path) -> Result<usize, String> {
    let mut misfit = None;
    let count = crate::clip::for_each_packet(path, |packet| {
        if packet.len() <= PACKET_BYTES {
            ring.push(packet);
        } else {
            misfit.get_or_insert(packet.len());
        }
    })?;
    match misfit {
        Some(size) => Err(format!("it has {size}-byte packets; Hindsight's are {PACKET_BYTES}")),
        None => Ok(count),
    }
}

fn encode(audio: Receiver<Audio>, ring: Arc<Mutex<Ring>>, input_rate: Arc<AtomicU32>, complexity: u8) {
    let mut encoder = encoder(complexity);
    let mut resampler: Option<(u32, Resampler)> = None;
    let mut pending: Vec<f32> = Vec::with_capacity(FRAME * 4);
    let mut packet = [0u8; 256];
    while let Ok(message) = audio.recv() {
        match message {
            Audio::Samples { rate, samples } => {
                if resampler.as_ref().map(|(current, _)| *current) != Some(rate) {
                    resampler = Some((rate, Resampler::new(rate, 48_000)));
                    input_rate.store(rate, Ordering::Relaxed);
                }
                resampler.as_mut().expect("resampler").1.push(&samples, &mut pending);
            }
            Audio::Gap(duration) => {
                let silent = (duration.as_secs_f64() * 48_000.0) as usize;
                pending.extend(std::iter::repeat_n(0.0, silent));
            }
        }
        while pending.len() >= FRAME {
            let size = encoder.encode_float_to_slice(&pending[..FRAME], &mut packet).expect("encode");
            ring.lock().expect("ring").push(&packet[..size]);
            pending.drain(..FRAME);
        }
    }
    // Leave nothing of the recording in the scratch buffers either.
    pending.fill(0.0);
    packet.fill(0);
}

fn control(
    mut settings: Settings,
    audio: Sender<Audio>,
    commands: Receiver<Command>,
    current_shared: Arc<Mutex<Option<(String, Role)>>>,
    on_event: impl Fn(Event),
) {
    let host = cpal::default_host();
    let (lost_sender, lost_receiver) = mpsc::channel::<String>();
    let mut switcher = Switcher::new(Instant::now());
    let mut current: Option<(cpal::Stream, Role, String, Heartbeat)> = None;
    // Time no microphone was recording, sent to the encoder as silence once one is again.
    let mut silent_since: Option<Instant> = None;
    let mut announced_wait = false;

    loop {
        // Commands first, so stopping is never held up by a microphone.
        loop {
            match commands.try_recv() {
                Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return,
                Ok(Command::SetMicrophones { primary, fallback }) => {
                    settings.primary = primary;
                    settings.fallback = fallback;
                    switcher = Switcher::new(Instant::now());
                    if current.take().is_some() {
                        silent_since = Some(Instant::now());
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        let now = Instant::now();
        let current_role = current.as_ref().map(|(_, role, _, _)| *role);
        let primary_listed = || settings.primary.as_deref().is_some_and(|name| find_device(&host, name).is_some());
        if switcher.should_open(now, current_role, settings.primary.is_some(), primary_listed) {
            let primary = if switcher.primary_allowed(now) { settings.primary.as_ref() } else { None };
            match choose_device(&host, primary, settings.fallback.as_ref()) {
                Some((device, role)) => {
                    let name = device_name(&device);
                    if current.take().is_some() {
                        silent_since = Some(Instant::now());
                    }
                    let heartbeat = Heartbeat::new();
                    match open_stream(&device, audio.clone(), lost_sender.clone(), heartbeat.clone(), settings.buffer_frames) {
                        Ok(stream) => {
                            // The watchdogs' clocks start once the stream runs: opening can take
                            // seconds (a phone's microphone has to wake), and that isn't silence.
                            heartbeat.beat();
                            heartbeat.heard(&[1.0]);
                            if let Some(since) = silent_since.take() {
                                let _ = audio.send(Audio::Gap(since.elapsed()));
                            }
                            switcher.opened(Instant::now(), role);
                            *current_shared.lock().expect("current") = Some((name.clone(), role));
                            on_event(Event::Recording { device: name.clone(), role });
                            announced_wait = false;
                            current = Some((stream, role, name, heartbeat));
                        }
                        Err(_) => announce_wait(&mut switcher, &mut announced_wait, &on_event),
                    }
                }
                None => announce_wait(&mut switcher, &mut announced_wait, &on_event),
            }
        }

        let reported = match lost_receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(reason) => Some(LossReason::Reported(reason)),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => None,
        };
        let verdict = reported.or_else(|| {
            current.as_ref().and_then(|(_, _, _, heartbeat)| watchdog(heartbeat.silent_for(), heartbeat.digital_silence_for()))
        });
        if let Some(reason) = verdict {
            if let Some((_, role, name, _)) = current.take() {
                silent_since = Some(Instant::now());
                *current_shared.lock().expect("current") = None;
                switcher.lost(Instant::now(), role);
                on_event(Event::Lost { device: name, role, reason });
            }
            while lost_receiver.try_recv().is_ok() {}
        }
    }
}

fn announce_wait(switcher: &mut Switcher, announced: &mut bool, on_event: &impl Fn(Event)) {
    let retry_in = switcher.failed(Instant::now());
    if !*announced || retry_in >= Duration::from_secs(8) {
        on_event(Event::Waiting { retry_in });
        *announced = true;
    }
}

/// No audio for 1.5 s, or nothing but exact zeros for 3 s, means the microphone is gone, even
/// when the system hasn't said so.
pub fn watchdog(silent_for: Duration, digital_silence_for: Duration) -> Option<LossReason> {
    if silent_for > Duration::from_millis(1500) {
        Some(LossReason::NoAudio)
    } else if digital_silence_for > Duration::from_secs(3) {
        Some(LossReason::OnlySilence)
    } else {
        None
    }
}

/// Decides when to open a microphone and which ones are allowed, with back-offs so a microphone
/// that keeps failing doesn't make recording bounce back and forth.
#[derive(Debug)]
pub struct Switcher {
    /// The chosen microphone isn't tried again before this, after it failed.
    retry_primary_at: Instant,
    primary_delay: Duration,
    /// Nothing is opened again before this, after every microphone failed or sent nothing.
    retry_any_at: Instant,
    failures_in_a_row: u32,
    last_primary_check: Instant,
    opened_at: Option<(Instant, Role)>,
}

const PRIMARY_FIRST_DELAY: Duration = Duration::from_secs(10);
const LONGEST_DELAY: Duration = Duration::from_secs(60);
const PRIMARY_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// A microphone that has worked this long resets the back-offs.
const STABLE_AFTER: Duration = Duration::from_secs(300);

impl Switcher {
    pub fn new(now: Instant) -> Self {
        Switcher {
            retry_primary_at: now,
            primary_delay: PRIMARY_FIRST_DELAY,
            retry_any_at: now,
            failures_in_a_row: 0,
            last_primary_check: now,
            opened_at: None,
        }
    }

    /// Whether to open a microphone now: when there's none (and no back-off is running), or to
    /// move back to the chosen one once it's listed again and its back-off is over.
    pub fn should_open(
        &mut self,
        now: Instant,
        current: Option<Role>,
        primary_configured: bool,
        primary_listed: impl FnOnce() -> bool,
    ) -> bool {
        match current {
            None => now >= self.retry_any_at,
            Some(Role::Primary) => false,
            Some(_) if !primary_configured => false,
            Some(_) => {
                if now.duration_since(self.last_primary_check) < PRIMARY_CHECK_INTERVAL || !self.primary_allowed(now) {
                    return false;
                }
                self.last_primary_check = now;
                primary_listed()
            }
        }
    }

    pub fn primary_allowed(&self, now: Instant) -> bool {
        now >= self.retry_primary_at
    }

    pub fn opened(&mut self, now: Instant, role: Role) {
        self.opened_at = Some((now, role));
    }

    /// A recording microphone was lost.
    pub fn lost(&mut self, now: Instant, role: Role) {
        let lasted = self.opened_at.take().map(|(at, _)| now.duration_since(at)).unwrap_or_default();
        if lasted >= STABLE_AFTER {
            self.primary_delay = PRIMARY_FIRST_DELAY;
            self.failures_in_a_row = 0;
        }
        if role == Role::Primary {
            self.retry_primary_at = now + self.primary_delay;
            self.primary_delay = (self.primary_delay * 2).min(LONGEST_DELAY);
        }
        // A microphone that died almost at once counts towards the general back-off too, so a
        // denied permission (silence from everything) doesn't reopen streams every few seconds.
        if lasted < Duration::from_secs(10) {
            self.failures_in_a_row += 1;
            self.retry_any_at = now + general_delay(self.failures_in_a_row);
        }
    }

    /// No microphone could be opened at all; returns how long until the next try.
    pub fn failed(&mut self, now: Instant) -> Duration {
        self.failures_in_a_row += 1;
        let delay = general_delay(self.failures_in_a_row);
        self.retry_any_at = now + delay;
        delay
    }
}

/// 2, 4, 8, 16, 32, then 60 seconds; the first two failures retry at once.
fn general_delay(failures: u32) -> Duration {
    if failures <= 2 {
        Duration::ZERO
    } else {
        Duration::from_secs(1u64 << (failures - 2).min(6)).min(LONGEST_DELAY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_can_start_with_a_clip() {
        let directory = std::env::temp_dir().join(format!("hindsight-start-with-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("earlier.opus");
        let packets: Vec<Vec<u8>> = (0..50u8).map(|value| vec![value; PACKET_BYTES]).collect();
        crate::clip::write(&path, packets.iter(), 48_000).unwrap();

        // A ring smaller than the clip keeps its newest packets, as if they'd just been recorded.
        let mut ring = Ring::new(30, PACKET_BYTES);
        assert_eq!(start_buffer(&mut ring, &path).unwrap(), 50);
        assert_eq!(ring.newest(100).collect::<Vec<_>>(), packets[20..].to_vec());

        // Packets too big for a slot are refused rather than cut short.
        let large = directory.join("large.opus");
        crate::clip::write(&large, [vec![1u8; PACKET_BYTES + 1]].iter(), 48_000).unwrap();
        assert!(start_buffer(&mut Ring::new(4, PACKET_BYTES), &large).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn watchdog_catches_starved_and_silent_streams() {
        assert_eq!(watchdog(Duration::from_millis(100), Duration::from_millis(100)), None);
        assert_eq!(watchdog(Duration::from_millis(1600), Duration::ZERO), Some(LossReason::NoAudio));
        assert_eq!(watchdog(Duration::ZERO, Duration::from_millis(3100)), Some(LossReason::OnlySilence));
    }

    #[test]
    fn opens_when_nothing_is_recording() {
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        assert!(switcher.should_open(start, None, true, || true));
    }

    #[test]
    fn never_leaves_the_chosen_microphone_while_it_works() {
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        assert!(!switcher.should_open(start + 10 * SECOND, Some(Role::Primary), true, || true));
    }

    #[test]
    fn backs_off_a_failing_microphone_longer_each_time() {
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        switcher.opened(start, Role::Primary);
        switcher.lost(start + 20 * SECOND, Role::Primary);
        let lost = start + 20 * SECOND;
        assert!(!switcher.primary_allowed(lost + 9 * SECOND));
        assert!(switcher.primary_allowed(lost + 10 * SECOND));

        switcher.opened(lost + 10 * SECOND, Role::Primary);
        let lost_again = lost + 30 * SECOND;
        switcher.lost(lost_again, Role::Primary);
        assert!(!switcher.primary_allowed(lost_again + 19 * SECOND));
        assert!(switcher.primary_allowed(lost_again + 20 * SECOND));

        // Capped at a minute.
        let mut at = lost_again;
        for _ in 0..6 {
            at += 70 * SECOND;
            switcher.opened(at, Role::Primary);
            at += 20 * SECOND;
            switcher.lost(at, Role::Primary);
        }
        assert!(switcher.primary_allowed(at + 60 * SECOND));
    }

    #[test]
    fn returns_to_the_chosen_microphone_once_its_back_off_is_over() {
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        switcher.opened(start, Role::Primary);
        let lost = start + 20 * SECOND;
        switcher.lost(lost, Role::Primary);
        switcher.opened(lost, Role::Fallback);
        // Listed again, but still backing off.
        assert!(!switcher.should_open(lost + 5 * SECOND, Some(Role::Fallback), true, || true));
        // Back-off over and listed: switch back.
        assert!(switcher.should_open(lost + 11 * SECOND, Some(Role::Fallback), true, || true));
        // Back-off over but not listed: stay.
        assert!(!switcher.should_open(lost + 14 * SECOND, Some(Role::Fallback), true, || false));
        // No chosen microphone at all: the fallback is as good as it gets.
        assert!(!switcher.should_open(lost + 20 * SECOND, Some(Role::Fallback), false, || true));
    }

    #[test]
    fn checks_for_the_chosen_microphone_at_most_every_two_seconds() {
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        let mut checks = 0;
        for tenth in 0..50u32 {
            let at = start + Duration::from_millis(u64::from(tenth) * 100);
            switcher.should_open(at, Some(Role::Fallback), true, || {
                checks += 1;
                false
            });
        }
        assert_eq!(checks, 2); // at 2 s and 4 s, not 50 times
    }

    #[test]
    fn a_microphone_that_dies_at_once_slows_retries_down() {
        // Like a denied microphone permission: everything opens, then sends only silence.
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        let mut at = start;
        let mut waits = Vec::new();
        for _ in 0..7 {
            switcher.opened(at, Role::SystemDefault);
            at += 3 * SECOND;
            switcher.lost(at, Role::SystemDefault);
            let wait = switcher.retry_any_at.saturating_duration_since(at);
            waits.push(wait.as_secs());
            at += wait;
        }
        assert_eq!(waits, vec![0, 0, 2, 4, 8, 16, 32]);
    }

    #[test]
    fn a_long_stable_run_resets_the_back_offs() {
        let start = Instant::now();
        let mut switcher = Switcher::new(start);
        for _ in 0..4 {
            switcher.opened(start, Role::Primary);
            switcher.lost(start + 20 * SECOND, Role::Primary);
        }
        let later = start + 1000 * SECOND;
        switcher.opened(later, Role::Primary);
        let lost = later + 400 * SECOND; // ran for over five minutes
        switcher.lost(lost, Role::Primary);
        assert!(switcher.primary_allowed(lost + 10 * SECOND), "the delay should be back to 10 s");
    }
}
