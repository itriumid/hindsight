//! The timeline: everything the buffer held at one moment, frozen and still encrypted, for
//! choosing exactly what to save. Recording carries on into the buffer meanwhile.
//!
//! It shows how loud each stretch was, plays from any moment, and saves any range as a clip.
//! Audio is decrypted a packet at a time and decoded as it's needed: three hours decoded at once
//! would be about 2 GB, and held decrypted, it would be the one copy of the audio that isn't
//! scrambled if it reached swap.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use opusic_c::{Channels, Decoder, SampleRate};

use crate::clip::{self, SAMPLES_PER_PACKET};
use crate::encoding::PACKET_SECONDS;
use crate::player::{Reader, Source};
use crate::ring::Frozen;

/// Packets decoded and thrown away before the one wanted, so the decoder has warmed up (as for a
/// clip, `clip::ClipReader`). Four packets is the 80 ms a clip's pre-skip allows for.
const WARM_UP_PACKETS: usize = 4;

/// A loudness bar measures its loudest stretch of this many packets (100 ms), so a few words in
/// an otherwise quiet minute still show.
const WINDOW_PACKETS: usize = 5;

/// Quieter than this counts as silence: a bar's level is never below it.
pub const SILENCE_DB: f32 = -100.0;

pub struct Timeline {
    frozen: Frozen,
    input_sample_rate: u32,
}

impl Timeline {
    pub fn new(frozen: Frozen, input_sample_rate: u32) -> Timeline {
        Timeline { frozen, input_sample_rate }
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frozen.len() as f64 * PACKET_SECONDS)
    }

    pub fn is_empty(&self) -> bool {
        self.frozen.is_empty()
    }

    /// The packet that `at` falls in, counted from the oldest.
    fn packet_at(&self, at: Duration) -> usize {
        ((at.as_nanos() * 48_000 / 1_000_000_000) as u64 / SAMPLES_PER_PACKET).min(self.frozen.len() as u64) as usize
    }

    /// How loud the timeline is along its length, as `bars` levels in decibels relative to full
    /// scale (0 is the loudest possible, `SILENCE_DB` the quietest). Each bar is its loudest
    /// 100 ms. Bars arrive newest first, since what someone wants is most likely recent, through
    /// `on_bar(index, level)`, where index 0 is the oldest; returning false stops early.
    pub fn levels(&self, bars: usize, mut on_bar: impl FnMut(usize, f32) -> bool) -> Result<(), String> {
        let packets = self.frozen.len();
        let bars = bars.min(packets);
        let mut samples = Vec::with_capacity(5760);
        for bar in (0..bars).rev() {
            let (start, end) = (bar * packets / bars, (bar + 1) * packets / bars);
            let mut decoder = PacketDecoder::new()?;
            for index in start.saturating_sub(WARM_UP_PACKETS)..start {
                samples.clear();
                decoder.decode(&self.frozen, index, &mut samples)?;
            }
            let mut loudest = 0f64;
            let (mut sum, mut counted, mut in_window) = (0f64, 0usize, 0usize);
            for index in start..end {
                samples.clear();
                decoder.decode(&self.frozen, index, &mut samples)?;
                sum += samples.iter().map(|&sample| f64::from(sample) * f64::from(sample)).sum::<f64>();
                counted += samples.len();
                in_window += 1;
                if in_window == WINDOW_PACKETS || index + 1 == end {
                    loudest = loudest.max(sum / counted.max(1) as f64);
                    (sum, counted, in_window) = (0.0, 0, 0);
                }
            }
            samples.fill(0.0);
            let level = if loudest > 0.0 { (10.0 * loudest.log10()) as f32 } else { SILENCE_DB };
            if !on_bar(bar, level.max(SILENCE_DB)) {
                break;
            }
        }
        Ok(())
    }

    /// Saves `start` to `end` as a clip at `path`, atomically, the way every clip is written.
    /// The clip also takes the 80 ms before `start`, which its pre-skip spends warming the
    /// decoder up, so playback begins right at `start`.
    pub fn save(&self, path: &Path, start: Duration, end: Duration) -> std::io::Result<usize> {
        let last = self.packet_at(end).max(self.packet_at(start));
        let first = self.packet_at(start).saturating_sub(WARM_UP_PACKETS).min(last);
        let packets = (first..last).map(|index| Wiped(self.frozen.with_packet(index, <[u8]>::to_vec)));
        clip::write(path, packets, self.input_sample_rate)
    }
}

/// A decrypted packet on its way into a clip, wiped once written.
struct Wiped(Vec<u8>);

impl AsRef<[u8]> for Wiped {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Wiped {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// Decodes frozen packets one at a time.
struct PacketDecoder {
    decoder: Decoder,
    frame: Vec<f32>,
}

impl PacketDecoder {
    fn new() -> Result<PacketDecoder, String> {
        let decoder = Decoder::new(Channels::Mono, SampleRate::Hz48000).map_err(|error| format!("{error:?}"))?;
        Ok(PacketDecoder { decoder, frame: vec![0f32; 5760] })
    }

    fn decode(&mut self, frozen: &Frozen, index: usize, out: &mut Vec<f32>) -> Result<(), String> {
        let decoded = frozen
            .with_packet(index, |packet| self.decoder.decode_float_to_slice(packet, &mut self.frame, false))
            .map_err(|error| format!("packet {index}: {error:?}"))?;
        out.extend_from_slice(&self.frame[..decoded]);
        Ok(())
    }
}

impl Drop for PacketDecoder {
    fn drop(&mut self) {
        self.frame.fill(0.0);
    }
}

/// Plays the timeline from a moment, for the player (`Player::play_timeline`).
pub struct TimelineReader {
    timeline: Arc<Timeline>,
    next: usize,
    decoder: PacketDecoder,
    /// Samples still to drop from the next packet, to start on the exact sample asked for.
    skip: usize,
}

impl Reader for TimelineReader {
    fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, String> {
        if self.next >= self.timeline.frozen.len() {
            return Ok(false);
        }
        let before = out.len();
        self.decoder.decode(&self.timeline.frozen, self.next, out)?;
        self.next += 1;
        let dropped = self.skip.min(out.len() - before);
        out.drain(before..before + dropped);
        self.skip -= dropped;
        Ok(true)
    }
}

impl Source for Timeline {
    fn length(&self) -> Duration {
        self.duration()
    }

    fn open_at(self: Arc<Self>, start: Duration) -> Result<Box<dyn Reader>, String> {
        let target = (start.as_nanos() * 48_000 / 1_000_000_000) as u64;
        let packet = self.packet_at(start);
        let mut decoder = PacketDecoder::new()?;
        let mut discard = Vec::new();
        for index in packet.saturating_sub(WARM_UP_PACKETS)..packet {
            decoder.decode(&self.frozen, index, &mut discard)?;
        }
        discard.fill(0.0);
        let skip = if packet < self.frozen.len() { (target - packet as u64 * SAMPLES_PER_PACKET) as usize } else { 0 };
        Ok(Box::new(TimelineReader { timeline: self, next: packet, decoder, skip }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::{FRAME, PACKET_BYTES, encoder};
    use crate::ring::Ring;

    /// A timeline of `seconds` per stretch: (seconds, amplitude of a 440 Hz tone; 0 is silence).
    fn timeline(stretches: &[(u32, f32)]) -> Arc<Timeline> {
        let packets: u32 = stretches.iter().map(|(seconds, _)| seconds * 50).sum();
        let mut ring = Ring::new(packets as usize, PACKET_BYTES);
        let mut encoder = encoder(5);
        let mut phase = 0f32;
        for &(seconds, amplitude) in stretches {
            for _ in 0..seconds * 50 {
                let frame: Vec<f32> = (0..FRAME)
                    .map(|_| {
                        phase += 440.0 / 48_000.0;
                        (phase * std::f32::consts::TAU).sin() * amplitude
                    })
                    .collect();
                let mut packet = [0u8; PACKET_BYTES];
                let size = encoder.encode_float_to_slice(&frame, &mut packet).unwrap();
                ring.push(&packet[..size]);
            }
        }
        Arc::new(Timeline::new(ring.freeze(ring.len()), 48_000))
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("hindsight-test-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn its_length_is_what_was_frozen() {
        assert_eq!(timeline(&[(7, 0.3)]).duration(), Duration::from_secs(7));
    }

    #[test]
    fn levels_show_speech_and_silence_newest_first() {
        // Ten seconds of tone, ten of silence, ten of tone, in one-second bars.
        let timeline = timeline(&[(10, 0.5), (10, 0.0), (10, 0.5)]);
        let mut bars = Vec::new();
        timeline.levels(30, |index, level| {
            bars.push((index, level));
            true
        })
        .unwrap();
        assert_eq!(bars.iter().map(|(index, _)| *index).collect::<Vec<_>>(), (0..30).rev().collect::<Vec<_>>(), "newest first");
        let level = |wanted: usize| bars.iter().find(|(index, _)| *index == wanted).unwrap().1;
        // A sine at 0.5 has an RMS of about -9 dB.
        for tone in (0..10).chain(20..30) {
            assert!(level(tone) > -15.0, "bar {tone} of the tone measured {}", level(tone));
        }
        // The encoder delays audio by about 6.5 ms, so the tone's last moment lands in the first
        // silent bar; the rest of the silence is silent.
        for quiet in 11..20 {
            assert!(level(quiet) < -60.0, "bar {quiet} of the silence measured {}", level(quiet));
        }
    }

    #[test]
    fn levels_stop_when_asked() {
        let timeline = timeline(&[(5, 0.3)]);
        let mut seen = 0;
        timeline.levels(5, |_, _| {
            seen += 1;
            seen < 2
        })
        .unwrap();
        assert_eq!(seen, 2);
    }

    #[test]
    fn reading_starts_on_the_exact_sample() {
        let timeline = timeline(&[(6, 0.3)]);
        let mut reader = Arc::clone(&timeline).open_at(Duration::from_millis(2_510)).unwrap();
        let mut samples = Vec::new();
        while reader.next(&mut samples).unwrap() {}
        assert_eq!(samples.len(), (6_000 - 2_510) * 48);
        assert!(samples.iter().any(|sample| sample.abs() > 0.1), "decoded silence");
    }

    #[test]
    fn saving_a_range_makes_a_clip_of_that_range() {
        let directory = scratch("timeline-save");
        let path = directory.join("range.opus");
        let timeline = timeline(&[(5, 0.0), (10, 0.4), (5, 0.0)]);
        timeline.save(&path, Duration::from_secs(5), Duration::from_secs(15)).unwrap();
        // The 80 ms taken from before the start are the clip's pre-skip, so it's 10 s exactly.
        assert_eq!(clip::duration(&path).unwrap(), Duration::from_secs(10));
        let decoded = clip::decode(&path).unwrap();
        // It starts on the tone straight away, not on the silence before it.
        assert!(decoded.samples[..4_800].iter().any(|sample| sample.abs() > 0.1), "starts silent");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_range_is_clamped_to_the_timeline() {
        let directory = scratch("timeline-clamp");
        let path = directory.join("all.opus");
        let timeline = timeline(&[(3, 0.3)]);
        timeline.save(&path, Duration::ZERO, Duration::from_secs(60)).unwrap();
        // From the very start there's nothing before it to warm up on, so the pre-skip eats the
        // first 80 ms, as it does for any clip.
        assert_eq!(clip::duration(&path).unwrap(), Duration::from_millis(2_920));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
