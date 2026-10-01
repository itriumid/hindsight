//! Plays a saved clip through the default speakers, streaming: a thread decodes a packet at a
//! time into about half a second of queue, and the audio output drains it. Even a three-hour
//! clip is never decoded whole.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::capture::Resampler;
use crate::clip::{ClipReader, duration};

enum Command {
    Play,
    Pause,
    Seek(Duration),
}

/// Where playback is, shared with the audio output.
#[derive(Default)]
struct Progress {
    /// The clip position (in 48 kHz samples) that the queue's first sample came from.
    base: AtomicU64,
    /// Output frames played since `base`, at the device's rate.
    frames: AtomicU64,
    playing: AtomicBool,
    finished: AtomicBool,
}

pub struct Player {
    path: PathBuf,
    length: Duration,
    output_rate: u32,
    progress: Arc<Progress>,
    commands: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Opens `path` and starts playing it straight away.
    pub fn play(path: &Path) -> Result<Player, String> {
        let length = duration(path)?;
        let device = cpal::default_host().default_output_device().ok_or("no speakers or headphones found")?;
        let config = device.default_output_config().map_err(|error| error.to_string())?;
        let output_rate = config.sample_rate();
        let progress = Arc::new(Progress::default());
        progress.playing.store(true, Ordering::Relaxed);
        let (commands, receiver) = mpsc::channel();
        let (ready_sender, ready) = mpsc::channel();
        let thread = {
            let path = path.to_path_buf();
            let progress = Arc::clone(&progress);
            std::thread::Builder::new()
                .name("hindsight-player".into())
                .spawn(move || run(path, device, config, progress, receiver, ready_sender))
                .map_err(|error| error.to_string())?
        };
        // The thread opens the output and the clip; wait for it to say whether that worked.
        ready.recv().map_err(|_| "the player stopped before starting".to_string())??;
        Ok(Player { path: path.to_path_buf(), length, output_rate, progress, commands: Some(commands), thread: Some(thread) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn length(&self) -> Duration {
        self.length
    }

    pub fn position(&self) -> Duration {
        let base = self.progress.base.load(Ordering::Relaxed);
        let frames = self.progress.frames.load(Ordering::Relaxed);
        let samples = base + frames * 48_000 / u64::from(self.output_rate);
        Duration::from_secs_f64(samples as f64 / 48_000.0).min(self.length)
    }

    pub fn is_playing(&self) -> bool {
        self.progress.playing.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.progress.finished.load(Ordering::Relaxed)
    }

    pub fn resume(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn seek(&self, to: Duration) {
        self.send(Command::Seek(to.min(self.length)));
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.commands.take(); // closing the channel ends the thread
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// About half a second of audio waiting for the output.
fn queue_target(output_rate: u32) -> usize {
    output_rate as usize / 2
}

fn run(
    path: PathBuf,
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    progress: Arc<Progress>,
    commands: Receiver<Command>,
    ready: Sender<Result<(), String>>,
) {
    let output_rate = config.sample_rate();
    let channels = config.channels() as usize;
    let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(queue_target(output_rate) * 2)));

    let stream = {
        let queue = Arc::clone(&queue);
        let progress = Arc::clone(&progress);
        let fill = move |output: &mut [f32]| {
            let mut queue = queue.lock().expect("queue");
            let mut played = 0;
            for frame in output.chunks_mut(channels) {
                let sample = queue.pop_front();
                if sample.is_some() {
                    played += 1;
                }
                frame.fill(sample.unwrap_or(0.0));
            }
            progress.frames.fetch_add(played, Ordering::Relaxed);
        };
        let built = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(config.into(), move |data: &mut [f32], _: &_| fill(data), |_| {}, None),
            // Linux sound cards often default to 16-bit integers.
            cpal::SampleFormat::I16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device.build_output_stream(
                    config.into(),
                    move |data: &mut [i16], _: &_| {
                        scratch.resize(data.len(), 0.0);
                        fill(&mut scratch);
                        for (out, sample) in data.iter_mut().zip(&scratch) {
                            *out = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    },
                    |_| {},
                    None,
                )
            }
            other => Err(cpal::Error::with_message(cpal::ErrorKind::UnsupportedConfig, format!("{other:?} output"))),
        };
        match built {
            Ok(stream) => stream,
            Err(error) => {
                let _ = ready.send(Err(format!("couldn't open the speakers: {error}")));
                return;
            }
        }
    };
    let mut reader = match ClipReader::open(&path) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if let Err(error) = stream.play() {
        let _ = ready.send(Err(error.to_string()));
        return;
    }
    let _ = ready.send(Ok(()));

    let mut resampler = Resampler::new(48_000, output_rate);
    let mut decoded = Vec::with_capacity(5760);
    let mut resampled = Vec::with_capacity(5760);
    let mut ended = false;
    loop {
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(Command::Play) => {
                if progress.finished.swap(false, Ordering::Relaxed) {
                    // Playing again after the end starts from the top.
                    restart(&path, Duration::ZERO, &mut reader, &queue, &progress, &mut resampler, output_rate);
                    ended = false;
                }
                progress.playing.store(true, Ordering::Relaxed);
                let _ = stream.play();
            }
            Ok(Command::Pause) => {
                progress.playing.store(false, Ordering::Relaxed);
                let _ = stream.pause();
            }
            Ok(Command::Seek(to)) => {
                restart(&path, to, &mut reader, &queue, &progress, &mut resampler, output_rate);
                progress.finished.store(false, Ordering::Relaxed);
                ended = false;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Keep the queue topped up while playing.
        while !ended && queue.lock().expect("queue").len() < queue_target(output_rate) {
            decoded.clear();
            match reader.next(&mut decoded) {
                Ok(true) => {
                    resampled.clear();
                    resampler.push(&decoded, &mut resampled);
                    queue.lock().expect("queue").extend(resampled.iter().copied());
                }
                Ok(false) | Err(_) => ended = true,
            }
        }
        if ended && progress.playing.load(Ordering::Relaxed) && queue.lock().expect("queue").is_empty() {
            progress.playing.store(false, Ordering::Relaxed);
            progress.finished.store(true, Ordering::Relaxed);
            let _ = stream.pause();
        }
    }
    // Decoded audio doesn't linger after playback.
    queue.lock().expect("queue").iter_mut().for_each(|sample| *sample = 0.0);
    decoded.fill(0.0);
    resampled.fill(0.0);
}

fn restart(
    path: &Path,
    to: Duration,
    reader: &mut ClipReader,
    queue: &Mutex<VecDeque<f32>>,
    progress: &Progress,
    resampler: &mut Resampler,
    output_rate: u32,
) {
    if let Ok(fresh) = ClipReader::open_at(path, to) {
        *reader = fresh;
        let mut queue = queue.lock().expect("queue");
        queue.clear();
        progress.base.store((to.as_nanos() * 48_000 / 1_000_000_000) as u64, Ordering::Relaxed);
        progress.frames.store(0, Ordering::Relaxed);
        *resampler = Resampler::new(48_000, output_rate);
    }
}
