//! The encoder settings every recording uses, and the buffer sized for them.

pub use opusic_c::Encoder;
use opusic_c::{Application, Bitrate, Channels, SampleRate, Signal};

use crate::ring::Ring;

pub const BITRATE: u32 = 16_000;
pub const PACKET_SECONDS: f64 = 0.02;
/// 20 ms at 48 kHz.
pub const FRAME: usize = 960;
/// Every packet's size, since the encoder runs at a hard constant bitrate: 40 bytes.
pub const PACKET_BYTES: usize = (BITRATE as f64 * PACKET_SECONDS / 8.0) as usize;
/// Blind listening tests couldn't tell complexity 5 from the maximum, 10, at less than half the
/// CPU.
pub const DEFAULT_COMPLEXITY: u8 = 5;

pub fn encoder(complexity: u8) -> Encoder {
    let mut encoder = Encoder::new(Channels::Mono, SampleRate::Hz48000, Application::Voip).expect("encoder");
    encoder.set_bitrate(Bitrate::Value(BITRATE)).expect("bitrate");
    // Hard constant bitrate: every packet is PACKET_BYTES, so the ring is fixed slots.
    encoder.set_vbr(false).expect("constant bitrate");
    encoder.set_signal(Signal::Voice).expect("signal");
    encoder.set_complexity(complexity).expect("complexity");
    encoder
}

/// A ring holding `buffer_minutes` of audio, locked in RAM where the system allows it.
pub fn ring_for(buffer_minutes: f64) -> Ring {
    let capacity = (buffer_minutes * 60.0 / PACKET_SECONDS).round() as usize;
    let mut ring = Ring::new(capacity, PACKET_BYTES);
    ring.lock();
    ring
}
