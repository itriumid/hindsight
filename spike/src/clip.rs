//! Saved clips: Opus packets in an Ogg container (RFC 7845), the standard `.opus` file.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opusic_c::{Channels, Decoder, SampleRate};

/// Opus always counts time in 48 kHz samples, whatever rate was encoded.
pub const SAMPLES_PER_PACKET: u64 = 960;

/// A clip cut from the middle of a stream starts with a decoder that hasn't warmed up, so the
/// format's guidance is to skip at least 80 ms (3840 samples) of its start.
const MID_STREAM_PRE_SKIP: u16 = 3840;

const SERIAL: u32 = 0x4869_6e64; // "Hind"

pub fn write<P: AsRef<[u8]>>(
    path: &Path,
    packets: impl Iterator<Item = P>,
    input_sample_rate: u32,
) -> std::io::Result<usize> {
    let mut writer = PacketWriter::new(BufWriter::new(File::create(path)?));
    writer.write_packet(opus_head(input_sample_rate).to_vec(), SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    writer.write_packet(opus_tags(), SERIAL, PacketWriteEndInfo::EndPage, 0)?;

    let packets: Vec<P> = packets.collect();
    let last = packets.len().saturating_sub(1);
    for (index, packet) in packets.iter().enumerate() {
        let granule = u64::from(MID_STREAM_PRE_SKIP) + (index as u64 + 1) * SAMPLES_PER_PACKET;
        let end = if index == last {
            PacketWriteEndInfo::EndStream
        } else if (index + 1) % 50 == 0 {
            PacketWriteEndInfo::EndPage // one page a second keeps seeking cheap
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        writer.write_packet(packet.as_ref().to_vec(), SERIAL, end, granule)?;
    }
    writer.inner_mut().flush()?;
    Ok(packets.len())
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
