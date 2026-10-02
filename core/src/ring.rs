//! The rolling buffer: a fixed array of Opus packets, oldest overwritten first, encrypted.
//!
//! The encoder runs in hard constant-bitrate mode, so every 20 ms packet has the same size and
//! the buffer is one allocation, sized once, that never grows.
//!
//! Every packet is encrypted with ChaCha20 under a random key made when the buffer is, so if the
//! operating system ever writes part of the buffer to swap, it writes only ciphertext. The key
//! itself (32 bytes) is locked in RAM, so it never reaches swap, and it's wiped when the buffer
//! is dropped or forgotten. The buffer is locked too where the system allows it (macOS and
//! Windows always; Linux only up to its memory lock limit, often 8 MB).
//!
//! A packet's nonce is its sequence number: the count of packets pushed before it under this
//! key. Sequence numbers only grow, even when the ring wraps or is cleared, so no nonce is ever
//! used twice with the same key.

use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher};

pub struct Ring {
    slots: Vec<u8>,
    lengths: Vec<u8>,
    slot_size: usize,
    capacity: usize,
    /// Packets pushed under the current key, which is also the next packet's sequence number.
    pushed: u64,
    /// How many slots hold a packet, up to `capacity`.
    filled: usize,
    key: Box<[u8; 32]>,
    pub locked: Locking,
    pub key_locked: Locking,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Locking {
    Locked,
    /// The operating system refused; the number is the error code it gave.
    Refused(i32),
    NotAttempted,
}

impl Ring {
    pub fn new(capacity: usize, slot_size: usize) -> Self {
        assert!(slot_size <= u8::MAX as usize, "a slot's length has to fit in a byte");
        let mut key = Box::new([0u8; 32]);
        let key_locked = lock_region(&key[..]);
        fill_random(&mut key[..]);
        Ring {
            // Zero-filled up front, so every page is really allocated before it's locked.
            slots: vec![0; capacity * slot_size],
            lengths: vec![0; capacity],
            slot_size,
            capacity,
            pushed: 0,
            filled: 0,
            key,
            locked: Locking::NotAttempted,
            key_locked,
        }
    }

    pub fn bytes(&self) -> usize {
        self.slots.len() + self.lengths.len() + self.key.len()
    }

    /// Asks the operating system to keep the buffer in RAM, never in swap.
    pub fn lock(&mut self) -> Locking {
        self.locked = lock_region(&self.slots).and(lock_region(&self.lengths));
        self.locked
    }

    pub fn push(&mut self, packet: &[u8]) {
        assert!(
            packet.len() <= self.slot_size,
            "packet of {} bytes doesn't fit a {}-byte slot",
            packet.len(),
            self.slot_size
        );
        let sequence = self.pushed;
        let slot = (sequence % self.capacity as u64) as usize;
        let start = slot * self.slot_size;
        let stored = &mut self.slots[start..start + packet.len()];
        stored.copy_from_slice(packet);
        cipher(&self.key, sequence).apply_keystream(stored);
        self.lengths[slot] = packet.len() as u8;
        self.pushed += 1;
        self.filled = (self.filled + 1).min(self.capacity);
    }

    pub fn len(&self) -> usize {
        self.filled
    }

    /// The newest `count` packets, oldest first, decrypted.
    pub fn newest(&self, count: usize) -> impl Iterator<Item = Vec<u8>> + '_ {
        let count = count.min(self.filled) as u64;
        let first = self.pushed - count;
        (first..self.pushed).map(move |sequence| {
            let slot = (sequence % self.capacity as u64) as usize;
            let start = slot * self.slot_size;
            let mut packet = self.slots[start..start + self.lengths[slot] as usize].to_vec();
            cipher(&self.key, sequence).apply_keystream(&mut packet);
            packet
        })
    }

    /// Forgets every packet and switches to a new key, for "forget the buffer".
    #[cfg_attr(not(test), allow(dead_code))] // the application's "Forget" uses it; the spike doesn't
    pub fn clear(&mut self) {
        self.slots.fill(0);
        self.lengths.fill(0);
        self.filled = 0;
        // A new key starts a new sequence, so restarting the count reuses no nonce.
        wipe(&mut self.key[..]);
        fill_random(&mut self.key[..]);
        self.pushed = 0;
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // Nothing recorded, and no key, should outlive the buffer in freed memory.
        self.slots.fill(0);
        wipe(&mut self.key[..]);
    }
}

/// A frozen copy of the newest packets, for working on a stretch of audio (the timeline) while
/// recording carries on. It stays encrypted, under its own copy of the buffer's key, and only
/// ever decrypts one packet at a time into a buffer that's wiped straight after, so the audio is
/// never held in the clear and no nonce is used for anything but decrypting what it was made for.
///
/// Only the key is locked in RAM: the packets are ciphertext, which may reach swap as safely as
/// the buffer's own, and locking 22 MB on every freeze would keep growing the working set Windows
/// reserves for the process.
pub struct Frozen {
    slots: Vec<u8>,
    lengths: Vec<u8>,
    slot_size: usize,
    /// The sequence number of the oldest packet, which with the key decrypts it.
    first: u64,
    key: Box<[u8; 32]>,
    pub key_locked: Locking,
}

impl Ring {
    /// The newest `count` packets as they are, encrypted, oldest first.
    pub fn freeze(&self, count: usize) -> Frozen {
        let count = count.min(self.filled);
        let first = self.pushed - count as u64;
        let mut key = Box::new([0u8; 32]);
        let key_locked = lock_region(&key[..]);
        key.copy_from_slice(&self.key[..]);
        let mut slots = vec![0; count * self.slot_size];
        let mut lengths = vec![0; count];
        for (index, (copy, length)) in slots.chunks_mut(self.slot_size).zip(lengths.iter_mut()).enumerate() {
            let slot = ((first + index as u64) % self.capacity as u64) as usize;
            let from = slot * self.slot_size;
            copy.copy_from_slice(&self.slots[from..from + self.slot_size]);
            *length = self.lengths[slot];
        }
        Frozen { slots, lengths, slot_size: self.slot_size, first, key, key_locked }
    }
}

impl Frozen {
    pub fn len(&self) -> usize {
        self.lengths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lengths.is_empty()
    }

    /// Hands packet `index` (0 is the oldest), decrypted, to `use_packet`, and wipes it as soon
    /// as that returns.
    pub fn with_packet<T>(&self, index: usize, use_packet: impl FnOnce(&[u8]) -> T) -> T {
        let mut buffer = [0u8; u8::MAX as usize];
        let length = self.lengths[index] as usize;
        let start = index * self.slot_size;
        buffer[..length].copy_from_slice(&self.slots[start..start + length]);
        cipher(&self.key, self.first + index as u64).apply_keystream(&mut buffer[..length]);
        let result = use_packet(&buffer[..length]);
        wipe(&mut buffer);
        result
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        self.slots.fill(0);
        wipe(&mut self.key[..]);
        if self.key_locked == Locking::Locked {
            unlock_region(&self.key[..]);
        }
    }
}

fn cipher(key: &[u8; 32], sequence: u64) -> ChaCha20 {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&sequence.to_le_bytes());
    ChaCha20::new(&(*key).into(), &nonce.into())
}

/// Zeroes memory in a way the compiler can't optimize away as a dead store.
fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(unix)]
fn fill_random(bytes: &mut [u8]) {
    // getentropy serves at most 256 bytes per call, from the kernel's secure generator.
    for chunk in bytes.chunks_mut(256) {
        let result = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
        assert_eq!(result, 0, "the system's random number generator failed");
    }
}

#[cfg(windows)]
fn fill_random(bytes: &mut [u8]) {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let status = unsafe {
        BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
    };
    assert_eq!(status, 0, "the system's random number generator failed");
}

#[cfg(unix)]
fn lock_region(region: &[u8]) -> Locking {
    let result = unsafe { libc::mlock(region.as_ptr().cast(), region.len()) };
    if result == 0 {
        Locking::Locked
    } else {
        Locking::Refused(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

/// Windows only locks pages within the process's minimum working set, so the working set grows
/// by the region's size first.
#[cfg(windows)]
fn lock_region(region: &[u8]) -> Locking {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Memory::VirtualLock;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessWorkingSetSize, SetProcessWorkingSetSize,
    };
    const SLACK: usize = 1 << 20;
    unsafe {
        let process = GetCurrentProcess();
        let (mut minimum, mut maximum) = (0usize, 0usize);
        if GetProcessWorkingSetSize(process, &mut minimum, &mut maximum) == 0
            || SetProcessWorkingSetSize(process, minimum + region.len() + SLACK, maximum + region.len() + SLACK) == 0
            || VirtualLock(region.as_ptr().cast(), region.len()) == 0
        {
            return Locking::Refused(GetLastError() as i32);
        }
    }
    Locking::Locked
}

#[cfg(not(any(unix, windows)))]
fn lock_region(_region: &[u8]) -> Locking {
    Locking::NotAttempted
}

/// Undoes `lock_region`, for memory that's about to be freed; on Windows that includes giving
/// back the working set it added.
#[cfg(unix)]
fn unlock_region(region: &[u8]) {
    unsafe { libc::munlock(region.as_ptr().cast(), region.len()) };
}

#[cfg(windows)]
fn unlock_region(region: &[u8]) {
    use windows_sys::Win32::System::Memory::VirtualUnlock;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessWorkingSetSize, SetProcessWorkingSetSize,
    };
    const SLACK: usize = 1 << 20;
    unsafe {
        VirtualUnlock(region.as_ptr().cast(), region.len());
        let process = GetCurrentProcess();
        let (mut minimum, mut maximum) = (0usize, 0usize);
        if GetProcessWorkingSetSize(process, &mut minimum, &mut maximum) != 0 {
            let grown = region.len() + SLACK;
            SetProcessWorkingSetSize(process, minimum.saturating_sub(grown), maximum.saturating_sub(grown));
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn unlock_region(_region: &[u8]) {}

impl Locking {
    fn and(self, other: Locking) -> Locking {
        match (self, other) {
            (Locking::Locked, Locking::Locked) => Locking::Locked,
            (Locking::Refused(code), _) | (_, Locking::Refused(code)) => Locking::Refused(code),
            _ => Locking::NotAttempted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packets(ring: &Ring, count: usize) -> Vec<Vec<u8>> {
        ring.newest(count).collect()
    }

    #[test]
    fn keeps_only_the_newest_packets_in_order() {
        let mut ring = Ring::new(3, 4);
        for value in 1..=5u8 {
            ring.push(&[value; 2]);
        }
        assert_eq!(packets(&ring, 10), vec![vec![3, 3], vec![4, 4], vec![5, 5]]);
        assert_eq!(packets(&ring, 2), vec![vec![4, 4], vec![5, 5]]);
    }

    #[test]
    fn stores_no_plaintext() {
        let mut ring = Ring::new(4, 40);
        let packet = [0xAB; 40];
        for _ in 0..4 {
            ring.push(&packet);
        }
        // Every packet is identical, yet none is stored as itself, and no two slots match.
        let slots: Vec<&[u8]> = ring.slots.chunks(40).collect();
        assert!(slots.iter().all(|slot| *slot != packet));
        for (index, slot) in slots.iter().enumerate() {
            assert!(slots[index + 1..].iter().all(|other| other != slot), "two slots share a keystream");
        }
        assert_eq!(packets(&ring, 4), vec![packet.to_vec(); 4]);
    }

    #[test]
    fn a_slot_reused_after_wrapping_gets_a_new_nonce() {
        let mut ring = Ring::new(1, 8);
        let packet = [7u8; 8];
        ring.push(&packet);
        let first = ring.slots.clone();
        ring.push(&packet); // same plaintext, same slot, one sequence number later
        assert_ne!(ring.slots, first, "the same slot reused its keystream");
        assert_eq!(packets(&ring, 1), vec![packet.to_vec()]);
    }

    #[test]
    fn forgetting_wipes_everything_and_changes_the_key() {
        let mut ring = Ring::new(3, 4);
        ring.push(&[9; 4]);
        let old_key = *ring.key;
        ring.clear();
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.newest(3).count(), 0);
        assert!(ring.slots.iter().all(|&byte| byte == 0));
        assert_ne!(*ring.key, old_key);
        ring.push(&[1, 2, 3]);
        assert_eq!(packets(&ring, 1), vec![vec![1, 2, 3]]);
    }

    #[test]
    fn a_frozen_copy_holds_the_newest_packets_still_encrypted() {
        let mut ring = Ring::new(4, 8);
        for value in 1..=6u8 {
            ring.push(&[value; 8]);
        }
        let frozen = ring.freeze(3);
        assert_eq!(frozen.len(), 3);
        let decrypted: Vec<Vec<u8>> = (0..3).map(|index| frozen.with_packet(index, <[u8]>::to_vec)).collect();
        assert_eq!(decrypted, packets(&ring, 3));
        // The copy is the ring's ciphertext, never the packets themselves.
        for (index, slot) in frozen.slots.chunks(8).enumerate() {
            assert_ne!(slot, decrypted[index].as_slice(), "packet {index} is stored in the clear");
        }
        // Asking for more than the ring holds freezes what there is.
        assert_eq!(ring.freeze(100).len(), 4);
    }

    #[test]
    fn a_frozen_copy_keeps_what_it_froze_while_recording_carries_on() {
        let mut ring = Ring::new(3, 4);
        ring.push(&[1; 4]);
        ring.push(&[2; 4]);
        let frozen = ring.freeze(2);
        for value in 3..=9u8 {
            ring.push(&[value; 4]); // wraps the ring twice over
        }
        ring.clear(); // and changes its key
        let kept: Vec<Vec<u8>> = (0..2).map(|index| frozen.with_packet(index, <[u8]>::to_vec)).collect();
        assert_eq!(kept, vec![vec![1; 4], vec![2; 4]]);
    }

    #[test]
    fn every_buffer_gets_its_own_key() {
        assert_ne!(*Ring::new(1, 1).key, *Ring::new(1, 1).key);
    }
}
