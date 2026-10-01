//! Hindsight spike: can a laptop keep hours of speech in a locked, in-memory buffer cheaply,
//! and save the last few minutes as a standard audio file? Three commands:
//!
//!   bench   [hours] [buffer-minutes] [save-minutes]   synthetic audio, as fast as possible
//!   devices                                           list microphones
//!   record  [seconds] [buffer-minutes] [save-minutes] [microphone] [fallback]
//!                                                     the real microphone, in real time;
//!                                                     microphones are part of a name from
//!                                                     `devices`. If the microphone drops,
//!                                                     recording moves to the fallback (or the
//!                                                     system default) and back when it returns
//!   compare [seconds] [microphone]                    record once without loss, then encode the
//!                                                     same speech at complexity 10, 5 and 3 for
//!                                                     a listening test
//!   verify  <clip.opus>                               decode a clip and write a WAV beside it
//!
//! Nothing here is the application; it's measurements to decide the application's design.

mod clip;
mod ring;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opusic_c::{Application, Bitrate, Channels, Encoder, SampleRate, Signal};
use ring::{Locking, Ring};

const BITRATE: u32 = 16_000;
const PACKET_SECONDS: f64 = 0.02;
const FRAME: usize = 960; // 20 ms at 48 kHz
const PACKET_BYTES: usize = (BITRATE as f64 * PACKET_SECONDS / 8.0) as usize; // 40

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let number = |index: usize, default: f64| {
        arguments.get(index).and_then(|value| value.parse().ok()).unwrap_or(default)
    };
    match arguments.first().map(String::as_str) {
        Some("bench") => bench(number(1, 3.0), number(2, 180.0), number(3, 15.0)),
        Some("devices") => devices(),
        Some("record") => record(number(1, 30.0), number(2, 180.0), number(3, 1.0), arguments.get(4), arguments.get(5)),
        Some("compare") => compare(number(1, 20.0), arguments.get(2)),
        Some("verify") => verify(Path::new(arguments.get(1).expect("verify needs a clip path"))),
        _ => eprintln!("usage: hindsight-spike bench|record|verify (see src/main.rs)"),
    }
}

fn encoder() -> Encoder {
    let mut encoder = Encoder::new(Channels::Mono, SampleRate::Hz48000, Application::Voip).expect("encoder");
    encoder.set_bitrate(Bitrate::Value(BITRATE)).expect("bitrate");
    // Hard constant bitrate: every packet is PACKET_BYTES, so the ring is fixed slots.
    encoder.set_vbr(false).expect("constant bitrate");
    encoder.set_signal(Signal::Voice).expect("signal");
    if let Some(complexity) = setting("HINDSIGHT_COMPLEXITY") {
        encoder.set_complexity(complexity as u8).expect("complexity");
    }
    encoder
}

/// Experiments are switched with environment variables, so commands stay the same:
/// HINDSIGHT_COMPLEXITY (0-10), HINDSIGHT_BUFFER_FRAMES (microphone wake-up size),
/// HINDSIGHT_SKIP_ENCODE=1 (capture only).
fn setting(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|value| value.parse().ok())
}

fn ring_for(buffer_minutes: f64) -> Ring {
    let capacity = (buffer_minutes * 60.0 / PACKET_SECONDS).round() as usize;
    let mut ring = Ring::new(capacity, PACKET_BYTES);
    ring.lock();
    ring
}

fn bench(hours: f64, buffer_minutes: f64, save_minutes: f64) {
    let mut ring = ring_for(buffer_minutes);
    let mut encoder = encoder();
    let mut packet = [0u8; 256];
    let mut frame = [0f32; FRAME];
    let mut voice = SyntheticVoice::default();
    let frames = (hours * 3600.0 / PACKET_SECONDS) as usize;
    let mut odd_sizes = 0usize;

    let started = Instant::now();
    let cpu_before = cpu_seconds();
    for _ in 0..frames {
        voice.fill(&mut frame);
        let size = encoder.encode_float_to_slice(&frame, &mut packet).expect("encode");
        if size != PACKET_BYTES {
            odd_sizes += 1;
        }
        ring.push(&packet[..size]);
    }
    let cpu = cpu_seconds() - cpu_before;
    let wall = started.elapsed().as_secs_f64();
    let audio = frames as f64 * PACKET_SECONDS;

    println!("== bench: {hours} h of synthetic speech into a {buffer_minutes} min buffer");
    println!("encoding: {:.1} s of CPU for {:.0} s of audio ({:.0}x real time)", cpu, audio, audio / wall);
    println!("  = {:.3}% of one core while recording live", cpu / audio * 100.0);
    report_buffer(&ring, odd_sizes);
    save_and_check(&ring, save_minutes, 48_000, "bench");
}

fn report_buffer(ring: &Ring, odd_sizes: usize) {
    println!(
        "buffer: {} packets held, {:.1} MB allocated once, locked in RAM: {:?}",
        ring.len(),
        ring.bytes() as f64 / 1_000_000.0,
        ring.locked
    );
    if let Locking::Refused(code) = ring.locked {
        println!("  lock refused: {}", std::io::Error::from_raw_os_error(code));
    }
    println!("  memory lock limit: {}", memory_lock_limit());
    println!("  packets not exactly {PACKET_BYTES} bytes: {odd_sizes}");
    println!("process peak memory: {:.1} MB", peak_memory_bytes() as f64 / 1_000_000.0);
}

fn save_and_check(ring: &Ring, save_minutes: f64, input_rate: u32, name: &str) {
    let count = (save_minutes * 60.0 / PACKET_SECONDS).round() as usize;
    let path = PathBuf::from(format!("{name}-last-{save_minutes}-min.opus"));
    let started = Instant::now();
    let written = clip::write(&path, ring.newest(count), input_rate).expect("write clip");
    let took = started.elapsed();
    let size = std::fs::metadata(&path).map(|metadata| metadata.len()).unwrap_or(0);
    println!(
        "saved {} ({} packets, {:.1} s of audio, {:.1} KB) in {:.0} ms",
        path.display(),
        written,
        written as f64 * PACKET_SECONDS,
        size as f64 / 1000.0,
        took.as_secs_f64() * 1000.0
    );
    verify(&path);
}

fn verify(path: &Path) {
    match clip::decode(path) {
        Ok(decoded) => {
            let wav = path.with_extension("wav");
            clip::write_wav(&wav, &decoded.samples).expect("write wav");
            let peak = decoded.samples.iter().fold(0f32, |peak, sample| peak.max(sample.abs()));
            println!(
                "verified: {} packets decode cleanly into {:.1} s (peak level {:.2}); listen: afplay {}",
                decoded.packets,
                decoded.samples.len() as f64 / 48_000.0,
                peak,
                wav.display()
            );
        }
        Err(error) => println!("VERIFY FAILED for {}: {error}", path.display()),
    }
}

fn device_name(device: &cpal::Device) -> String {
    device.description().map(|description| description.name().to_string()).unwrap_or_else(|_| "(unnamed)".into())
}

fn devices() {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|device| device_name(&device));
    for device in host.input_devices().expect("couldn't list microphones") {
        let name = device_name(&device);
        let config = device
            .default_input_config()
            .map(|config| format!("{} Hz, {} channel(s)", config.sample_rate(), config.channels()))
            .unwrap_or_else(|error| format!("unusable: {error}"));
        let marker = if Some(&name) == default.as_ref() { "*" } else { " " };
        println!("{marker} {name}  ({config})");
    }
    println!("* = system default");
}

/// The first microphone whose name contains `wanted`, ignoring case.
fn find_device(host: &cpal::Host, wanted: &str) -> Option<cpal::Device> {
    let wanted = wanted.to_lowercase();
    host.input_devices().ok()?.find(|device| device_name(device).to_lowercase().contains(&wanted))
}

/// The microphone to use now: the chosen one, else the fallback, else the system default.
fn choose_device(host: &cpal::Host, primary: Option<&String>, fallback: Option<&String>) -> Option<(cpal::Device, Role)> {
    if let Some(device) = primary.and_then(|name| find_device(host, name)) {
        return Some((device, Role::Primary));
    }
    if let Some(device) = fallback.and_then(|name| find_device(host, name)) {
        return Some((device, Role::Fallback));
    }
    host.default_input_device().map(|device| (device, Role::SystemDefault))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Primary,
    Fallback,
    SystemDefault,
}

/// What the encoding thread is sent.
enum Audio {
    Samples { rate: u32, samples: Vec<f32> },
    /// Time no microphone was recording, kept as silence so a clip's timeline matches real time.
    Gap(Duration),
}

/// Milliseconds since `epoch` at which audio last arrived, for the watchdog.
#[derive(Clone)]
struct Heartbeat {
    epoch: Instant,
    last: Arc<AtomicU64>,
    /// When a sample that wasn't exactly zero last arrived. A live microphone always has some
    /// noise floor, so a stream of perfect zeros is dead even though it keeps delivering.
    last_sound: Arc<AtomicU64>,
}

impl Heartbeat {
    fn new() -> Self {
        Heartbeat { epoch: Instant::now(), last: Arc::new(AtomicU64::new(0)), last_sound: Arc::new(AtomicU64::new(0)) }
    }
    fn beat(&self) {
        self.last.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    fn heard(&self, samples: &[f32]) {
        if samples.iter().any(|&sample| sample != 0.0) {
            self.last_sound.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
    }
    fn digital_silence_for(&self) -> Duration {
        Duration::from_millis(self.epoch.elapsed().as_millis() as u64 - self.last_sound.load(Ordering::Relaxed))
    }
    fn silent_for(&self) -> Duration {
        Duration::from_millis(self.epoch.elapsed().as_millis() as u64 - self.last.load(Ordering::Relaxed))
    }
}

fn open_stream(
    device: &cpal::Device,
    audio: mpsc::Sender<Audio>,
    lost: mpsc::Sender<()>,
    heartbeat: Heartbeat,
) -> Result<cpal::Stream, String> {
    let config = device.default_input_config().map_err(|error| error.to_string())?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let mut stream_config: cpal::StreamConfig = config.clone().into();
    if let Some(frames) = setting("HINDSIGHT_BUFFER_FRAMES") {
        if let cpal::SupportedBufferSize::Range { min, max } = config.buffer_size() {
            stream_config.buffer_size = cpal::BufferSize::Fixed(frames.clamp(*min, *max));
            println!("  buffer: {} frames (device allows {min} to {max})", frames.clamp(*min, *max));
        }
    }
    let on_error = move |error: cpal::Error| {
        eprintln!("  stream error: {error}");
        let _ = lost.send(());
    };
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            {
                let heartbeat = heartbeat.clone();
                move |data: &[f32], _: &_| {
                    heartbeat.beat();
                    let samples = downmix(data, channels, |sample| sample);
                    heartbeat.heard(&samples);
                    let _ = audio.send(Audio::Samples { rate, samples });
                }
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _: &_| {
                heartbeat.beat();
                let samples = downmix(data, channels, |sample| sample as f32 / i16::MAX as f32);
                heartbeat.heard(&samples);
                let _ = audio.send(Audio::Samples { rate, samples });
            },
            on_error,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|error| error.to_string())?;
    stream.play().map_err(|error| error.to_string())?;
    Ok(stream)
}

fn record(seconds: f64, buffer_minutes: f64, save_minutes: f64, primary: Option<&String>, fallback: Option<&String>) {
    let host = cpal::default_host();
    println!(
        "== record: {seconds} s; microphone {}, fallback {}",
        primary.map(String::as_str).unwrap_or("(system default)"),
        fallback.map(String::as_str).unwrap_or("(system default)")
    );

    let (audio_sender, audio_receiver) = mpsc::channel::<Audio>();
    let encoding = std::thread::spawn(move || {
        let mut ring = ring_for(buffer_minutes);
        let mut encoder = encoder();
        let mut resampler: Option<(u32, Resampler)> = None;
        let mut pending: Vec<f32> = Vec::with_capacity(FRAME * 4);
        let mut packet = [0u8; 256];
        let mut odd_sizes = 0;
        let mut last_rate = 48_000;
        let skip_encode = setting("HINDSIGHT_SKIP_ENCODE") == Some(1);
        while let Ok(message) = audio_receiver.recv() {
            match message {
                Audio::Samples { rate, samples } => {
                    if resampler.as_ref().map(|(current, _)| *current) != Some(rate) {
                        resampler = Some((rate, Resampler::new(rate, 48_000)));
                    }
                    last_rate = rate;
                    resampler.as_mut().unwrap().1.push(&samples, &mut pending);
                }
                Audio::Gap(duration) => {
                    let silent = (duration.as_secs_f64() * 48_000.0) as usize;
                    pending.extend(std::iter::repeat_n(0.0, silent));
                }
            }
            if skip_encode {
                pending.clear();
            }
            while pending.len() >= FRAME {
                let size = encoder.encode_float_to_slice(&pending[..FRAME], &mut packet).expect("encode");
                if size != PACKET_BYTES {
                    odd_sizes += 1;
                }
                ring.push(&packet[..size]);
                pending.drain(..FRAME);
            }
        }
        (ring, odd_sizes, last_rate)
    });

    let cpu_before = cpu_seconds();
    let started = Instant::now();
    let deadline = started + Duration::from_secs_f64(seconds);
    let (lost_sender, lost_receiver) = mpsc::channel::<()>();
    let mut current: Option<(cpal::Stream, Role, String)> = None;
    let mut silent_since: Option<Instant> = Some(started);
    let mut last_check = Instant::now();
    let mut switches = 0;
    let mut heartbeat = Heartbeat::new();
    // Some devices go quiet without reporting an error; no audio this long counts as lost.
    let watchdog = Duration::from_millis(1500);
    // After the chosen microphone fails, wait before trying it again, longer each time, so a
    // phone that stays listed while silent doesn't bounce recording back and forth.
    let mut retry_primary_at = Instant::now();
    let mut retry_delay = Duration::from_secs(10);

    let elapsed = |at: Instant| at.duration_since(started).as_secs_f64();
    while Instant::now() < deadline {
        // Open a microphone when there's none, or move back to the chosen one when it returns.
        let wants_switch = match &current {
            None => true,
            Some((_, role, _)) => {
                *role != Role::Primary
                    && last_check.elapsed() > Duration::from_secs(2)
                    && Instant::now() >= retry_primary_at
                    && primary.and_then(|name| find_device(&host, name)).is_some()
            }
        };
        if current.is_some() && last_check.elapsed() > Duration::from_secs(2) {
            last_check = Instant::now();
        }
        if wants_switch {
            let primary_now = if Instant::now() >= retry_primary_at { primary } else { None };
            if let Some((device, role)) = choose_device(&host, primary_now, fallback) {
                let name = device_name(&device);
                let now = Instant::now();
                // Stop the old stream first, then count the time until the new one runs as a gap.
                if current.take().is_some() {
                    silent_since = Some(now);
                }
                let fresh = Heartbeat::new();
                fresh.beat(); // the clock starts at opening, so a stream that never delivers counts
                fresh.heard(&[1.0]);
                match open_stream(&device, audio_sender.clone(), lost_sender.clone(), fresh.clone()) {
                    Ok(stream) => {
                        heartbeat = fresh;
                        if let Some(since) = silent_since.take() {
                            let gap = since.elapsed();
                            if gap > Duration::from_millis(30) && switches > 0 {
                                let _ = audio_sender.send(Audio::Gap(gap));
                            }
                        }
                        println!("  {:5.1} s: recording from {name} ({role:?})", elapsed(Instant::now()));
                        current = Some((stream, role, name));
                        switches += 1;
                    }
                    Err(error) => eprintln!("  couldn't open {name}: {error}"),
                }
            }
            last_check = Instant::now();
        }
        // A lost stream: drop it now, so the next pass opens the fallback.
        let reported = lost_receiver.recv_timeout(Duration::from_millis(100)).is_ok();
        let starved = current.is_some() && heartbeat.silent_for() > watchdog;
        let dead = current.is_some() && heartbeat.digital_silence_for() > Duration::from_secs(3);
        if starved {
            println!("  {:5.1} s: no audio for {:.1} s, treating it as lost", elapsed(Instant::now()), heartbeat.silent_for().as_secs_f64());
        } else if dead {
            println!("  {:5.1} s: only exact zeros for {:.1} s, treating it as lost", elapsed(Instant::now()), heartbeat.digital_silence_for().as_secs_f64());
        }
        let silent = starved || dead;
        if reported || silent {
            if let Some((_, role, name)) = current.take() {
                println!("  {:5.1} s: lost {name} ({role:?})", elapsed(Instant::now()));
                silent_since = Some(Instant::now());
                if role == Role::Primary {
                    retry_primary_at = Instant::now() + retry_delay;
                    println!("         won't try it again for {} s", retry_delay.as_secs());
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                }
            }
            while lost_receiver.try_recv().is_ok() {}
        }
    }
    drop(current);
    drop(audio_sender); // closes the channel, which ends the encoding thread
    let (ring, odd_sizes, rate) = encoding.join().expect("encoding thread");
    let wall = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu_before;

    println!("CPU: {:.2} s over {:.1} s = {:.2}% of one core (capture + encoding)", cpu, wall, cpu / wall * 100.0);
    println!(
        "timeline: {:.1} s of audio for {:.1} s of wall time (gaps kept as silence)",
        ring.len() as f64 * PACKET_SECONDS,
        wall
    );
    report_buffer(&ring, odd_sizes);
    save_and_check(&ring, save_minutes, rate, "record");
}

/// Records raw audio once, then encodes the identical samples at several complexities, so a
/// listening test compares only the encoder setting, not two different takes.
fn compare(seconds: f64, microphone: Option<&String>) {
    let host = cpal::default_host();
    let (device, role) = choose_device(&host, microphone, None).expect("no microphone found");
    println!("== compare: {seconds} s from {} ({role:?}). Speak now.", device_name(&device));
    let (audio_sender, audio_receiver) = mpsc::channel::<Audio>();
    let (lost_sender, _lost_receiver) = mpsc::channel::<()>();
    let stream = open_stream(&device, audio_sender, lost_sender, Heartbeat::new()).expect("couldn't open the microphone");
    std::thread::sleep(Duration::from_secs_f64(seconds));
    drop(stream);

    let mut raw = Vec::new();
    let mut rate = 48_000;
    let mut resampler: Option<Resampler> = None;
    while let Ok(Audio::Samples { rate: chunk_rate, samples }) = audio_receiver.try_recv() {
        rate = chunk_rate;
        resampler.get_or_insert_with(|| Resampler::new(chunk_rate, 48_000)).push(&samples, &mut raw);
    }
    println!("captured {:.1} s at {rate} Hz", raw.len() as f64 / 48_000.0);
    clip::write_wav(Path::new("compare-original.wav"), &raw).expect("write original");

    for complexity in [10u32, 5, 3] {
        let mut encoder = encoder();
        encoder.set_complexity(complexity as u8).expect("complexity");
        let mut ring = Ring::new(raw.len() / FRAME + 1, PACKET_BYTES);
        let mut packet = [0u8; 256];
        let cpu_before = cpu_seconds();
        for frame in raw.chunks_exact(FRAME) {
            let size = encoder.encode_float_to_slice(frame, &mut packet).expect("encode");
            ring.push(&packet[..size]);
        }
        let cpu = cpu_seconds() - cpu_before;
        let path = PathBuf::from(format!("compare-complexity-{complexity}.opus"));
        clip::write(&path, ring.newest(ring.len()), rate).expect("write clip");
        let decoded = clip::decode(&path).expect("decode");
        clip::write_wav(&path.with_extension("wav"), &decoded.samples).expect("write wav");
        println!(
            "complexity {complexity:2}: {:.3}% of one core to encode; listen: afplay {}",
            cpu / (raw.len() as f64 / 48_000.0) * 100.0,
            path.with_extension("wav").display()
        );
    }
}

fn downmix<T: Copy>(data: &[T], channels: usize, to_float: impl Fn(T) -> f32) -> Vec<f32> {
    data.chunks(channels)
        .map(|frame| frame.iter().map(|&sample| to_float(sample)).sum::<f32>() / channels as f32)
        .collect()
}

/// Linear interpolation; good enough to measure with, not to ship.
struct Resampler {
    step: f64,
    position: f64,
    previous: f32,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Resampler { step: from as f64 / to as f64, position: 0.0, previous: 0.0 }
    }

    fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < f64::EPSILON {
            output.extend_from_slice(input);
            return;
        }
        // position runs from -1 (the previous chunk's last sample) through the new samples.
        while self.position < input.len() as f64 - 1.0 {
            let index = self.position.floor();
            let fraction = (self.position - index) as f32;
            let left = if index < 0.0 { self.previous } else { input[index as usize] };
            let right = input[(index + 1.0) as usize];
            output.push(left + (right - left) * fraction);
            self.position += self.step;
        }
        self.position -= input.len() as f64;
        if let Some(&last) = input.last() {
            self.previous = last;
        }
    }
}

/// Voiced, syllable-shaped noise: a pitch that drifts, harmonics, four syllables a second and
/// pauses, so the encoder works about as hard as it would on speech.
#[derive(Default)]
struct SyntheticVoice {
    time: f64,
    phase: f64,
    seed: u32,
}

impl SyntheticVoice {
    fn fill(&mut self, frame: &mut [f32]) {
        for sample in frame.iter_mut() {
            let pitch = 140.0 + 40.0 * (self.time * 0.7).sin();
            self.phase += pitch / 48_000.0;
            let syllable = ((self.time * 4.0 * std::f64::consts::TAU).sin() * 0.5 + 0.5).powi(2);
            let talking = if (self.time / 6.0).fract() < 0.8 { 1.0 } else { 0.0 };
            let mut voiced = 0.0;
            for harmonic in 1..=8 {
                voiced += (self.phase * harmonic as f64 * std::f64::consts::TAU).sin() / harmonic as f64;
            }
            self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (self.seed >> 8) as f64 / (1u32 << 24) as f64 - 0.5;
            *sample = ((voiced * 0.25 * syllable * talking) + noise * 0.01) as f32;
            self.time += 1.0 / 48_000.0;
        }
    }
}

fn cpu_seconds() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

fn peak_memory_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // macOS reports bytes; Linux reports kilobytes.
    if cfg!(target_os = "macos") { usage.ru_maxrss as u64 } else { usage.ru_maxrss as u64 * 1024 }
}

fn memory_lock_limit() -> String {
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut limit) } != 0 {
        return "unknown".into();
    }
    let describe = |value: libc::rlim_t| {
        if value == libc::RLIM_INFINITY { "unlimited".to_string() } else { format!("{:.1} MB", value as f64 / 1_000_000.0) }
    };
    format!("{} (hard {})", describe(limit.rlim_cur), describe(limit.rlim_max))
}
