//! Microphones: finding and choosing one, and turning its stream into mono samples, with a
//! heartbeat the watchdog can read to notice a microphone that has quietly stopped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub fn device_name(device: &cpal::Device) -> String {
    device.description().map(|description| description.name().to_string()).unwrap_or_else(|_| "(unnamed)".into())
}

/// The first microphone whose name contains `wanted`, ignoring case.
pub fn find_device(host: &cpal::Host, wanted: &str) -> Option<cpal::Device> {
    let wanted = wanted.to_lowercase();
    host.input_devices().ok()?.find(|device| device_name(device).to_lowercase().contains(&wanted))
}

/// The microphone to use now: the chosen one, else the fallback, else the system default.
pub fn choose_device(host: &cpal::Host, primary: Option<&String>, fallback: Option<&String>) -> Option<(cpal::Device, Role)> {
    if let Some(device) = primary.and_then(|name| find_device(host, name)) {
        return Some((device, Role::Primary));
    }
    if let Some(device) = fallback.and_then(|name| find_device(host, name)) {
        return Some((device, Role::Fallback));
    }
    host.default_input_device().map(|device| (device, Role::SystemDefault))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    Primary,
    Fallback,
    SystemDefault,
}

/// What the encoding thread is sent.
pub enum Audio {
    Samples { rate: u32, samples: Vec<f32> },
    /// Time no microphone was recording, kept as silence so a clip's timeline matches real time.
    Gap(Duration),
}

/// Milliseconds since `epoch` at which audio last arrived, for the watchdog.
#[derive(Clone)]
pub struct Heartbeat {
    epoch: Instant,
    last: Arc<AtomicU64>,
    /// When a sample that wasn't exactly zero last arrived. A live microphone always has some
    /// noise floor, so a stream of perfect zeros is dead even though it keeps delivering.
    last_sound: Arc<AtomicU64>,
}

impl Heartbeat {
    pub fn new() -> Self {
        Heartbeat { epoch: Instant::now(), last: Arc::new(AtomicU64::new(0)), last_sound: Arc::new(AtomicU64::new(0)) }
    }
    pub fn beat(&self) {
        self.last.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    pub fn heard(&self, samples: &[f32]) {
        if samples.iter().any(|&sample| sample != 0.0) {
            self.last_sound.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
    }
    pub fn digital_silence_for(&self) -> Duration {
        Duration::from_millis(self.epoch.elapsed().as_millis() as u64 - self.last_sound.load(Ordering::Relaxed))
    }
    pub fn silent_for(&self) -> Duration {
        Duration::from_millis(self.epoch.elapsed().as_millis() as u64 - self.last.load(Ordering::Relaxed))
    }
}

pub fn open_stream(
    device: &cpal::Device,
    audio: mpsc::Sender<Audio>,
    lost: mpsc::Sender<String>,
    heartbeat: Heartbeat,
    buffer_frames: Option<u32>,
) -> Result<cpal::Stream, String> {
    let config = device.default_input_config().map_err(|error| error.to_string())?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let mut stream_config: cpal::StreamConfig = config.clone().into();
    if let Some(frames) = buffer_frames {
        if let cpal::SupportedBufferSize::Range { min, max } = config.buffer_size() {
            stream_config.buffer_size = cpal::BufferSize::Fixed(frames.clamp(*min, *max));
        }
    }
    let on_error = move |error: cpal::Error| {
        let _ = lost.send(error.to_string());
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

pub fn downmix<T: Copy>(data: &[T], channels: usize, to_float: impl Fn(T) -> f32) -> Vec<f32> {
    data.chunks(channels)
        .map(|frame| frame.iter().map(|&sample| to_float(sample)).sum::<f32>() / channels as f32)
        .collect()
}

/// Linear interpolation; good enough to measure with, not to ship.
pub struct Resampler {
    step: f64,
    position: f64,
    previous: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Resampler { step: from as f64 / to as f64, position: 0.0, previous: 0.0 }
    }

    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
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
