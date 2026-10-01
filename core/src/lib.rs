//! Hindsight's recording core: microphones, encoding, the encrypted rolling buffer, saved clips,
//! and keeping the process's memory out of crash dumps. The application and the measurement
//! spike both build on it.

pub mod capture;
pub mod clip;
pub mod encoding;
pub mod naming;
pub mod player;
pub mod privacy;
pub mod recorder;
pub mod ring;
