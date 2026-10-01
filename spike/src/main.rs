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
//!   seek    <clip.opus> <seconds>                     time opening a clip at a position
//!   verify  <clip.opus>                               decode a clip and write a WAV beside it
//!
//! Nothing here is the application; it's measurements to decide the application's design. The
//! recording itself lives in `hindsight-core`, which the application uses too.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use hindsight_core::capture::{Audio, Heartbeat, Resampler, choose_device, device_name, open_stream};
use hindsight_core::encoding::{DEFAULT_COMPLEXITY, FRAME, PACKET_BYTES, PACKET_SECONDS, ring_for};
use hindsight_core::recorder::{Event, Recorder, Settings, Snapshot};
use hindsight_core::ring::{Locking, Ring};
use hindsight_core::{clip, privacy};

fn main() {
    let hardened = privacy::keep_memory_out_of_crash_dumps();
    println!("process: {}", if hardened.is_empty() { "crash dumps NOT restricted".to_string() } else { hardened.join(", ") });
    match clip::sweep_partials(Path::new(".")) {
        Ok(0) => {}
        Ok(removed) => println!("removed {removed} half-written clip(s) left by a crash"),
        Err(error) => eprintln!("couldn't check for half-written clips: {error}"),
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let number = |index: usize, default: f64| {
        arguments.get(index).and_then(|value| value.parse().ok()).unwrap_or(default)
    };
    match arguments.first().map(String::as_str) {
        Some("bench") => bench(number(1, 3.0), number(2, 180.0), number(3, 15.0)),
        Some("devices") => devices(),
        Some("record") => record(number(1, 30.0), number(2, 180.0), number(3, 1.0), arguments.get(4), arguments.get(5)),
        Some("compare") => compare(number(1, 20.0), arguments.get(2)),
        Some("seek") => seek(Path::new(arguments.get(1).expect("seek needs a clip path")), number(2, 0.0)),
        Some("verify") => verify(Path::new(arguments.get(1).expect("verify needs a clip path"))),
        _ => eprintln!("usage: hindsight-spike bench|record|verify (see src/main.rs)"),
    }
}

/// Experiments are switched with environment variables, so commands stay the same:
/// HINDSIGHT_COMPLEXITY (0-10), HINDSIGHT_BUFFER_FRAMES (microphone wake-up size),
/// HINDSIGHT_SKIP_ENCODE=1 (capture only).
fn setting(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|value| value.parse().ok())
}

fn encoder() -> hindsight_core::encoding::Encoder {
    hindsight_core::encoding::encoder(setting("HINDSIGHT_COMPLEXITY").map_or(DEFAULT_COMPLEXITY, |value| value as u8))
}

fn stream(device: &cpal::Device, audio: mpsc::Sender<Audio>, lost: mpsc::Sender<String>, heartbeat: Heartbeat) -> Result<cpal::Stream, String> {
    let frames = setting("HINDSIGHT_BUFFER_FRAMES");
    if let Some(frames) = frames {
        println!("  asking for {frames}-frame wake-ups");
    }
    open_stream(device, audio, lost, heartbeat, frames)
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
    println!("  encrypted with ChaCha20; its key is locked in RAM: {:?}", ring.key_locked);
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

fn save_snapshot(snapshot: &Snapshot, save_minutes: f64, name: &str) {
    let path = PathBuf::from(format!("{name}-last-{save_minutes}-min.opus"));
    let started = Instant::now();
    let written = clip::write(&path, snapshot.packets.iter(), snapshot.input_sample_rate).expect("write clip");
    println!(
        "saved {} ({} packets, {:.1} s of audio) in {:.0} ms",
        path.display(),
        written,
        snapshot.duration().as_secs_f64(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    verify(&path);
}

fn seek(path: &Path, seconds: f64) {
    let started = Instant::now();
    let length = clip::duration(path).expect("duration");
    let measured = started.elapsed();
    let started = Instant::now();
    let mut reader = clip::ClipReader::open_at(path, Duration::from_secs_f64(seconds)).expect("open");
    let mut samples = Vec::new();
    reader.next(&mut samples).expect("decode");
    println!(
        "{} is {:.1} s long (read in {:.1} ms); opened at {seconds} s and decoded the first packet in {:.0} ms",
        path.display(),
        length.as_secs_f64(),
        measured.as_secs_f64() * 1000.0,
        started.elapsed().as_secs_f64() * 1000.0
    );
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

fn record(seconds: f64, buffer_minutes: f64, save_minutes: f64, primary: Option<&String>, fallback: Option<&String>) {
    println!(
        "== record: {seconds} s; microphone {}, fallback {}",
        primary.map(String::as_str).unwrap_or("(system default)"),
        fallback.map(String::as_str).unwrap_or("(system default)")
    );
    let settings = Settings {
        primary: primary.cloned(),
        fallback: fallback.cloned(),
        buffer_minutes,
        complexity: setting("HINDSIGHT_COMPLEXITY").map_or(DEFAULT_COMPLEXITY, |value| value as u8),
        buffer_frames: setting("HINDSIGHT_BUFFER_FRAMES"),
    };
    let cpu_before = cpu_seconds();
    let started = Instant::now();
    let recorder = Recorder::start(settings, move |event| {
        let at = started.elapsed().as_secs_f64();
        match event {
            Event::Recording { device, role } => println!("  {at:5.1} s: recording from {device} ({role:?})"),
            Event::Lost { device, role, reason } => println!("  {at:5.1} s: lost {device} ({role:?}): {reason:?}"),
            Event::Waiting { retry_in } => println!("  {at:5.1} s: no microphone; trying again in {} s", retry_in.as_secs()),
        }
    });
    std::thread::sleep(Duration::from_secs_f64(seconds));
    let buffered = recorder.buffered();
    let snapshot = recorder.snapshot(Duration::from_secs_f64(save_minutes * 60.0));
    recorder.with_ring(|ring| report_buffer(ring, 0));
    drop(recorder); // stops the microphone and both threads
    let wall = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu_before;

    println!("CPU: {:.2} s over {:.1} s = {:.2}% of one core (capture + encoding)", cpu, wall, cpu / wall * 100.0);
    println!("timeline: {:.1} s of audio for {:.1} s of wall time (gaps kept as silence)", buffered.as_secs_f64(), wall);
    save_snapshot(&snapshot, save_minutes, "record");
}

/// Records raw audio once, then encodes the identical samples at several complexities, so a
/// listening test compares only the encoder setting, not two different takes.
fn compare(seconds: f64, microphone: Option<&String>) {
    let host = cpal::default_host();
    let (device, role) = choose_device(&host, microphone, None).expect("no microphone found");
    println!("== compare: {seconds} s from {} ({role:?}). Speak now.", device_name(&device));
    let (audio_sender, audio_receiver) = mpsc::channel::<Audio>();
    let (lost_sender, _lost_receiver) = mpsc::channel::<String>();
    let stream = stream(&device, audio_sender, lost_sender, Heartbeat::new()).expect("couldn't open the microphone");
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

#[cfg(unix)]
fn cpu_seconds() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

#[cfg(unix)]
fn peak_memory_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // macOS reports bytes; Linux reports kilobytes.
    if cfg!(target_os = "macos") { usage.ru_maxrss as u64 } else { usage.ru_maxrss as u64 * 1024 }
}

#[cfg(unix)]
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

#[cfg(windows)]
fn cpu_seconds() -> f64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero(), zero(), zero(), zero());
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) };
    // FILETIME counts 100-nanosecond intervals.
    let seconds = |time: FILETIME| ((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)) as f64 / 1e7;
    seconds(kernel) + seconds(user)
}

#[cfg(windows)]
fn peak_memory_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    counters.PeakWorkingSetSize as u64
}

#[cfg(windows)]
fn memory_lock_limit() -> String {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessWorkingSetSize};
    let (mut minimum, mut maximum) = (0usize, 0usize);
    unsafe { GetProcessWorkingSetSize(GetCurrentProcess(), &mut minimum, &mut maximum) };
    format!(
        "working set {:.1} to {:.1} MB (locked pages must fit the minimum)",
        minimum as f64 / 1e6,
        maximum as f64 / 1e6
    )
}
