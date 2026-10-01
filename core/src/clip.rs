//! Saved clips: Opus packets in an Ogg container (RFC 7845), the standard `.opus` file.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opusic_c::{Channels, Decoder, SampleRate};

/// Opus always counts time in 48 kHz samples, whatever rate was encoded.
pub const SAMPLES_PER_PACKET: u64 = 960;

/// A clip cut from the middle of a stream starts with a decoder that hasn't warmed up, so the
/// format's guidance is to skip at least 80 ms (3840 samples) of its start.
const MID_STREAM_PRE_SKIP: u16 = 3840;

const SERIAL: u32 = 0x4869_6e64; // "Hind"

/// Suffix of a clip still being written. A crash can only ever leave one of these behind, never
/// a half-written clip under its real name, and `sweep_partials` removes them.
pub const PARTIAL_SUFFIX: &str = ".hindsight-partial";

/// Writes a clip atomically: into a hidden temporary file beside it, flushed to disk, then
/// renamed into place in one step. The clip exists completely or not at all.
pub fn write<P: AsRef<[u8]>>(
    path: &Path,
    packets: impl Iterator<Item = P>,
    input_sample_rate: u32,
) -> std::io::Result<usize> {
    let mut partial = Partial::beside(path);
    let mut writer = PacketWriter::new(BufWriter::new(File::create(&partial.path)?));
    writer.write_packet(opus_head(input_sample_rate).to_vec(), SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    writer.write_packet(opus_tags(), SERIAL, PacketWriteEndInfo::EndPage, 0)?;

    let packets: Vec<P> = packets.collect();
    let last = packets.len().saturating_sub(1);
    for (index, packet) in packets.iter().enumerate() {
        // RFC 7845: a page's granule position counts every sample decoded so far, the pre-skip
        // included, so the clip's length is the last granule minus the pre-skip.
        let granule = (index as u64 + 1) * SAMPLES_PER_PACKET;
        let end = if index == last {
            PacketWriteEndInfo::EndStream
        } else if (index + 1) % 50 == 0 {
            PacketWriteEndInfo::EndPage // one page a second keeps seeking cheap
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        writer.write_packet(packet.as_ref().to_vec(), SERIAL, end, granule)?;
    }
    let file = writer.into_inner().into_inner().map_err(|error| error.into_error())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial.path, path)?;
    partial.committed = true;
    sync_directory(path);
    Ok(packets.len())
}

/// Deletes clips a crash left half-written in `directory`; returns how many.
pub fn sweep_partials(directory: &Path) -> std::io::Result<usize> {
    let mut removed = 0;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let is_partial = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(PARTIAL_SUFFIX));
        if is_partial && path.is_file() {
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// The temporary file behind a clip being written, deleted if the write doesn't finish, even
/// when it ends in a panic.
struct Partial {
    path: PathBuf,
    committed: bool,
}

impl Partial {
    fn beside(path: &Path) -> Self {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("clip");
        let directory = path.parent().unwrap_or(Path::new("."));
        Partial { path: directory.join(format!(".{name}{PARTIAL_SUFFIX}")), committed: false }
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Makes the rename itself durable on Unix; Windows has no directory handle to flush.
fn sync_directory(path: &Path) {
    #[cfg(unix)]
    if let Some(directory) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        if let Ok(handle) = File::open(directory) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn opus_head(input_sample_rate: u32) -> [u8; 19] {
    let mut head = [0u8; 19];
    head[..8].copy_from_slice(b"OpusHead");
    head[8] = 1; // version
    head[9] = 1; // mono
    head[10..12].copy_from_slice(&MID_STREAM_PRE_SKIP.to_le_bytes());
    head[12..16].copy_from_slice(&input_sample_rate.to_le_bytes());
    // Output gain 0 and channel mapping family 0 are already zero.
    head
}

fn opus_tags() -> Vec<u8> {
    let vendor = b"Hindsight spike";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // no user comments
    tags
}

/// Reads a clip a packet at a time, so even a three-hour clip is never decoded whole: playing and
/// exporting stream through this.
pub struct ClipReader {
    reader: ogg::reading::PacketReader<BufReader<File>>,
    decoder: Decoder,
    /// Samples still to drop from the front: the format's pre-skip, only at the very start.
    skip: usize,
    frame: Vec<f32>,
}

/// Packets decoded and thrown away before a seek target, so the decoder has warmed up by the
/// time the listener hears anything.
const SEEK_WARM_UP_PACKETS: u64 = 4;

impl ClipReader {
    pub fn open(path: &Path) -> Result<ClipReader, String> {
        ClipReader::open_at(path, Duration::ZERO)
    }

    /// Opens a clip positioned at `start`. Every packet is 20 ms, so the packet holding `start`
    /// is plain arithmetic; the ones before it are skipped without decoding.
    pub fn open_at(path: &Path, start: Duration) -> Result<ClipReader, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let mut reader = ogg::reading::PacketReader::new(BufReader::new(file));
        let head = reader.read_packet().map_err(|error| error.to_string())?.ok_or("empty clip")?;
        if !head.data.starts_with(b"OpusHead") || head.data.len() < 19 {
            return Err("not an Opus clip".into());
        }
        let pre_skip = u64::from(u16::from_le_bytes([head.data[10], head.data[11]]));
        let tags = reader.read_packet().map_err(|error| error.to_string())?.ok_or("clip has no tags")?;
        if !tags.data.starts_with(b"OpusTags") {
            return Err("missing OpusTags".into());
        }
        let decoder = Decoder::new(Channels::Mono, SampleRate::Hz48000).map_err(|error| format!("{error:?}"))?;
        let mut clip = ClipReader { reader, decoder, skip: pre_skip as usize, frame: vec![0f32; 5760] };

        // In whole numbers: 2.51 s in floating point is 120,479.999… samples, one short.
        let target_sample = (start.as_nanos() * 48_000 / 1_000_000_000) as u64 + pre_skip;
        let target_packet = target_sample / SAMPLES_PER_PACKET;
        if target_packet > 0 {
            let warm_up_from = target_packet.saturating_sub(SEEK_WARM_UP_PACKETS);
            for _ in 0..warm_up_from {
                if clip.reader.read_packet().map_err(|error| error.to_string())?.is_none() {
                    break;
                }
            }
            let mut discard = Vec::new();
            for _ in warm_up_from..target_packet {
                if !clip.next(&mut discard)? {
                    break;
                }
            }
            // Land on the exact sample, not just the packet's start.
            clip.skip = (target_sample % SAMPLES_PER_PACKET) as usize;
        }
        Ok(clip)
    }

    /// Decodes the next packet onto `out`; false once the clip has ended.
    pub fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, String> {
        let Some(packet) = self.reader.read_packet().map_err(|error| error.to_string())? else {
            return Ok(false);
        };
        let decoded = self
            .decoder
            .decode_float_to_slice(&packet.data, &mut self.frame, false)
            .map_err(|error| format!("{error:?}"))?;
        let dropped = self.skip.min(decoded);
        self.skip -= dropped;
        out.extend_from_slice(&self.frame[dropped..decoded]);
        Ok(true)
    }
}

/// A clip's length, from the granule position on its last page: instant, whatever its size.
pub fn duration(path: &Path) -> Result<Duration, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut head = [0u8; 64];
    let read = file.read(&mut head).map_err(|error| error.to_string())?;
    let at = find(&head[..read], b"OpusHead").ok_or("not an Opus clip")?;
    let pre_skip = u64::from(u16::from_le_bytes([head[at + 10], head[at + 11]]));

    let length = file.seek(SeekFrom::End(0)).map_err(|error| error.to_string())?;
    let tail_length = length.min(65_536);
    file.seek(SeekFrom::Start(length - tail_length)).map_err(|error| error.to_string())?;
    let mut tail = vec![0u8; tail_length as usize];
    file.read_exact(&mut tail).map_err(|error| error.to_string())?;
    let last_page = rfind(&tail, b"OggS").ok_or("no Ogg page in the clip's end")?;
    if last_page + 14 > tail.len() {
        return Err("truncated last page".into());
    }
    let granule = u64::from_le_bytes(tail[last_page + 6..last_page + 14].try_into().expect("eight bytes"));
    Ok(Duration::from_secs_f64(granule.saturating_sub(pre_skip) as f64 / 48_000.0))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|window| window == needle)
}

/// Writes a clip as a 16-bit, 48 kHz WAV for apps that can't open Opus, streaming, and as
/// atomically as a clip: a hidden partial file renamed into place.
pub fn export_wav(source: &Path, destination: &Path) -> Result<(), String> {
    let mut partial = Partial::beside(destination);
    let mut out = BufWriter::new(File::create(&partial.path).map_err(|error| error.to_string())?);
    out.write_all(&wav_header(0)).map_err(|error| error.to_string())?; // sizes filled in at the end
    let mut reader = ClipReader::open(source)?;
    let mut samples = Vec::with_capacity(5760);
    let mut written: u64 = 0;
    while reader.next(&mut samples)? {
        for sample in samples.drain(..) {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            out.write_all(&value.to_le_bytes()).map_err(|error| error.to_string())?;
        }
        written = out.stream_position().map_err(|error| error.to_string())?;
    }
    let data_bytes = written.saturating_sub(44);
    if data_bytes > u64::from(u32::MAX - 36) {
        return Err("too long for a WAV file (over 6 hours at 48 kHz)".into());
    }
    out.seek(SeekFrom::Start(0)).map_err(|error| error.to_string())?;
    out.write_all(&wav_header(data_bytes as u32)).map_err(|error| error.to_string())?;
    let file = out.into_inner().map_err(|error| error.into_error().to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    std::fs::rename(&partial.path, destination).map_err(|error| error.to_string())?;
    partial.committed = true;
    sync_directory(destination);
    Ok(())
}

fn wav_header(data_bytes: u32) -> [u8; 44] {
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    header[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    header[28..32].copy_from_slice(&96_000u32.to_le_bytes()); // bytes per second
    header[32..34].copy_from_slice(&2u16.to_le_bytes()); // block align
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    header
}

pub struct Decoded {
    pub packets: usize,
    pub samples: Vec<f32>,
}

/// Decodes every packet of a clip, which fails loudly on anything malformed.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let mut reader = ogg::reading::PacketReader::new(BufReader::new(
        File::open(path).map_err(|error| error.to_string())?,
    ));
    let mut decoder = Decoder::new(Channels::Mono, SampleRate::Hz48000).map_err(|error| format!("{error:?}"))?;
    let mut samples = Vec::new();
    let mut frame = vec![0f32; 5760]; // the longest Opus frame, 120 ms
    let mut index = 0;
    let mut packets = 0;
    while let Some(packet) = reader.read_packet().map_err(|error| error.to_string())? {
        match index {
            0 if !packet.data.starts_with(b"OpusHead") => return Err("missing OpusHead".into()),
            1 if !packet.data.starts_with(b"OpusTags") => return Err("missing OpusTags".into()),
            0 | 1 => {}
            _ => {
                let decoded = decoder
                    .decode_float_to_slice(&packet.data, &mut frame, false)
                    .map_err(|error| format!("packet {packets}: {error:?}"))?;
                samples.extend_from_slice(&frame[..decoded]);
                packets += 1;
            }
        }
        index += 1;
    }
    let skip = usize::from(MID_STREAM_PRE_SKIP).min(samples.len());
    samples.drain(..skip);
    Ok(Decoded { packets, samples })
}

/// A 16-bit mono WAV at 48 kHz, so a clip can be checked by ear with any player.
pub fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    let data_bytes = (samples.len() * 2) as u32;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&1u16.to_le_bytes())?; // mono
    out.write_all(&48_000u32.to_le_bytes())?;
    out.write_all(&96_000u32.to_le_bytes())?; // bytes per second
    out.write_all(&2u16.to_le_bytes())?; // block align
    out.write_all(&16u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())?;
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.write_all(&value.to_le_bytes())?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("hindsight-test-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A real clip of `seconds`: a 440 Hz tone, so decoded audio isn't silence.
    fn tone_clip(directory: &Path, seconds: u32) -> PathBuf {
        let mut encoder = opusic_c::Encoder::new(Channels::Mono, SampleRate::Hz48000, opusic_c::Application::Voip).unwrap();
        let mut phase = 0f32;
        let packets: Vec<Vec<u8>> = (0..seconds * 50)
            .map(|_| {
                let frame: Vec<f32> = (0..SAMPLES_PER_PACKET)
                    .map(|_| {
                        phase += 440.0 / 48_000.0;
                        (phase * std::f32::consts::TAU).sin() * 0.3
                    })
                    .collect();
                let mut packet = vec![0u8; 256];
                let size = encoder.encode_float_to_slice(&frame, &mut packet).unwrap();
                packet.truncate(size);
                packet
            })
            .collect();
        let path = directory.join("tone.opus");
        write(&path, packets.into_iter(), 48_000).unwrap();
        path
    }

    #[test]
    fn duration_comes_from_the_last_page() {
        let directory = scratch("duration");
        let path = tone_clip(&directory, 12);
        let length = duration(&path).unwrap().as_secs_f64();
        // 600 packets of 20 ms, less the 80 ms pre-skip.
        assert!((length - 11.92).abs() < 0.001, "{length}");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reading_streams_the_whole_clip() {
        let directory = scratch("stream");
        let path = tone_clip(&directory, 5);
        let mut reader = ClipReader::open(&path).unwrap();
        let mut samples = Vec::new();
        while reader.next(&mut samples).unwrap() {}
        let expected = (duration(&path).unwrap().as_secs_f64() * 48_000.0).round() as usize;
        assert_eq!(samples.len(), expected);
        assert!(samples.iter().any(|sample| sample.abs() > 0.1), "decoded silence");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn opening_at_a_time_starts_exactly_there() {
        let directory = scratch("seek");
        let path = tone_clip(&directory, 6);
        let total = (duration(&path).unwrap().as_secs_f64() * 48_000.0).round() as usize;
        let mut reader = ClipReader::open_at(&path, Duration::from_millis(2_510)).unwrap();
        let mut samples = Vec::new();
        while reader.next(&mut samples).unwrap() {}
        assert_eq!(samples.len(), total - 2_510 * 48, "should hold everything after 2.51 s");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_a_wav_with_every_sample() {
        let directory = scratch("wav");
        let path = tone_clip(&directory, 3);
        let wav = directory.join("tone.wav");
        export_wav(&path, &wav).unwrap();
        let bytes = std::fs::read(&wav).unwrap();
        let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 44 + data);
        let expected = (duration(&path).unwrap().as_secs_f64() * 48_000.0).round() as usize;
        assert_eq!(data / 2, expected);
        assert_eq!(names(&directory), vec!["tone.opus", "tone.wav"]);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_finished_write_leaves_only_the_clip() {
        let directory = scratch("finished");
        let path = directory.join("clip.opus");
        let mut encoder = opusic_c::Encoder::new(Channels::Mono, SampleRate::Hz48000, opusic_c::Application::Voip).unwrap();
        let silence = [0f32; SAMPLES_PER_PACKET as usize];
        let packets: Vec<Vec<u8>> = (0..2)
            .map(|_| {
                let mut packet = vec![0u8; 256];
                let size = encoder.encode_float_to_slice(&silence, &mut packet).unwrap();
                packet.truncate(size);
                packet
            })
            .collect();
        write(&path, packets.into_iter(), 48_000).unwrap();
        assert_eq!(names(&directory), vec!["clip.opus"]);
        assert_eq!(decode(&path).unwrap().packets, 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_write_that_panics_leaves_nothing() {
        let directory = scratch("panic");
        let path = directory.join("clip.opus");
        let packets = (0..10).map(|index| if index == 3 { panic!("interrupted") } else { [0u8; 40] });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&path, packets, 48_000)));
        assert!(result.is_err());
        assert!(names(&directory).is_empty(), "left behind: {:?}", names(&directory));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sweeping_removes_only_half_written_clips() {
        let directory = scratch("sweep");
        std::fs::write(directory.join("kept.opus"), b"a saved clip").unwrap();
        std::fs::write(directory.join(format!(".lost.opus{PARTIAL_SUFFIX}")), b"half").unwrap();
        assert_eq!(sweep_partials(&directory).unwrap(), 1);
        assert_eq!(names(&directory), vec!["kept.opus"]);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
