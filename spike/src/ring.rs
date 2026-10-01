//! The rolling buffer: a fixed array of Opus packets, oldest overwritten first.
//!
//! The encoder runs in hard constant-bitrate mode, so every 20 ms packet has the same size and
//! the buffer is one allocation, sized once, that never grows. That makes its memory
//! predictable and lets the whole thing be locked in RAM, so the operating system never writes
//! it to swap.

pub struct Ring {
    slots: Vec<u8>,
    lengths: Vec<u8>,
    slot_size: usize,
    capacity: usize,
    /// Index of the slot the next packet goes into.
    next: usize,
    /// How many slots hold a packet, up to `capacity`.
    filled: usize,
    pub locked: Locking,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Locking {
    Locked,
    /// The operating system refused; the number is the errno it gave.
    Refused(i32),
    NotAttempted,
}

impl Ring {
    pub fn new(capacity: usize, slot_size: usize) -> Self {
        assert!(slot_size <= u8::MAX as usize, "a slot's length has to fit in a byte");
        Ring {
            // Zero-filled up front, so every page is really allocated before it's locked.
            slots: vec![0; capacity * slot_size],
            lengths: vec![0; capacity],
            slot_size,
            capacity,
            next: 0,
            filled: 0,
            locked: Locking::NotAttempted,
        }
    }

    pub fn bytes(&self) -> usize {
        self.slots.len() + self.lengths.len()
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
        let start = self.next * self.slot_size;
        self.slots[start..start + packet.len()].copy_from_slice(packet);
        self.lengths[self.next] = packet.len() as u8;
        self.next = (self.next + 1) % self.capacity;
        self.filled = (self.filled + 1).min(self.capacity);
    }

    pub fn len(&self) -> usize {
        self.filled
    }

    /// The newest `count` packets, oldest first.
    pub fn newest(&self, count: usize) -> impl Iterator<Item = &[u8]> {
        let count = count.min(self.filled);
        let first = (self.next + self.capacity - count) % self.capacity;
        (0..count).map(move |offset| {
            let index = (first + offset) % self.capacity;
            let start = index * self.slot_size;
            &self.slots[start..start + self.lengths[index] as usize]
        })
    }

    /// Overwrites every packet, for "forget the buffer".
    pub fn clear(&mut self) {
        self.slots.fill(0);
        self.lengths.fill(0);
        self.next = 0;
        self.filled = 0;
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // Nothing recorded should outlive the buffer in freed memory.
        self.clear();
    }
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

    #[test]
    fn keeps_only_the_newest_packets_in_order() {
        let mut ring = Ring::new(3, 4);
        for value in 1..=5u8 {
            ring.push(&[value; 2]);
        }
        let newest: Vec<Vec<u8>> = ring.newest(10).map(|packet| packet.to_vec()).collect();
        assert_eq!(newest, vec![vec![3, 3], vec![4, 4], vec![5, 5]]);
        let last_two: Vec<Vec<u8>> = ring.newest(2).map(|packet| packet.to_vec()).collect();
        assert_eq!(last_two, vec![vec![4, 4], vec![5, 5]]);
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut ring = Ring::new(3, 4);
        ring.push(&[9; 4]);
        ring.clear();
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.newest(3).count(), 0);
        assert!(ring.slots.iter().all(|&byte| byte == 0));
    }
}
