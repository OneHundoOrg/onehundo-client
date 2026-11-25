//! Plane A POSIX Shared Memory (`shm_open` + `mmap`) Ring Consumer.
//! Enforces `FREE (0) -> WRITING (1) -> READY (2) -> READING (3) -> FREE (0)`
//! with Acquire/Release atomic ordering matching `onehundo_internal.h`.

use crate::core::types::LifecycleSignal;
use crate::ipc::wire::WIRE_MAGIC;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

pub const RING_SLOT_COUNT: usize = 4;
pub const RING_SLOT_CAPACITY: usize = 64 * 1024;

pub const SLOT_STATE_FREE: u32 = 0;
pub const SLOT_STATE_WRITING: u32 = 1;
pub const SLOT_STATE_READY: u32 = 2;
pub const SLOT_STATE_READING: u32 = 3;

#[repr(C)]
pub struct SnapshotSlotHeader {
    pub struct_size: u32,
    pub api_version: u32,
    pub slot_index: u32,
    pub slot_state: AtomicU32,
    pub sequence_number: u64,
    pub timestamp_nanos: u64,
    pub lifecycle_signal: u32,
    pub frame_gap_count: u32,
    pub payload_size_bytes: u64,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct ArenaSlot {
    pub header: SnapshotSlotHeader,
    pub payload: [u8; RING_SLOT_CAPACITY],
}

#[repr(C)]
pub struct ArenaShmLayout {
    pub magic: u32,
    pub version: u32,
    pub slot_count: u32,
    pub slot_capacity: u32,
    pub slots: [ArenaSlot; RING_SLOT_COUNT],
}

#[derive(Debug, Clone)]
pub struct AcquiredSnapshot {
    pub slot_index: usize,
    pub sequence_number: u64,
    pub timestamp_nanos: u64,
    pub lifecycle_signal: LifecycleSignal,
    pub frame_gap_count: u32,
    pub payload: Vec<u8>,
}

#[cfg(unix)]
mod posix_ffi {
    use super::*;
    pub const O_RDWR: c_int = 2;
    pub const PROT_READ: c_int = 1;
    pub const PROT_WRITE: c_int = 2;
    pub const MAP_SHARED: c_int = 1;
    pub const MAP_FAILED: *mut c_void = !0usize as *mut c_void;

    extern "C" {
        pub fn shm_open(name: *const c_char, oflag: c_int, mode: u32) -> c_int;
        pub fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: isize,
        ) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
        pub fn close(fd: c_int) -> c_int;
    }
}

pub struct ShmArenaConsumer {
    #[cfg(unix)]
    ptr: *mut ArenaShmLayout,
    #[cfg(unix)]
    fd: i32,
    pub shm_name: String,
    cursor: usize,
}

unsafe impl Send for ShmArenaConsumer {}

impl ShmArenaConsumer {
    #[cfg(unix)]
    pub fn open(shm_name: &str) -> Result<Self, String> {
        // Validate shm_name starts with '/' and has no traversal
        if !shm_name.starts_with('/') || shm_name.contains("..") {
            return Err(format!("invalid POSIX shm_name '{}'", shm_name));
        }
        let c_name = CString::new(shm_name).map_err(|_| "invalid CString shm_name".to_string())?;
        let size = std::mem::size_of::<ArenaShmLayout>();

        // Note: slot_state atomic lives inside the shared header, so the consumer requires
        // PROT_READ | PROT_WRITE to transition READY -> READING -> FREE; payload is never mutated.
        unsafe {
            let fd = posix_ffi::shm_open(c_name.as_ptr(), posix_ffi::O_RDWR, 0);
            if fd < 0 {
                return Err(format!(
                    "shm_open('{}') failed: {}",
                    shm_name,
                    std::io::Error::last_os_error()
                ));
            }
            let mapped = posix_ffi::mmap(
                ptr::null_mut(),
                size,
                posix_ffi::PROT_READ | posix_ffi::PROT_WRITE,
                posix_ffi::MAP_SHARED,
                fd,
                0,
            );
            if mapped == posix_ffi::MAP_FAILED {
                posix_ffi::close(fd);
                return Err(format!(
                    "mmap('{}') failed: {}",
                    shm_name,
                    std::io::Error::last_os_error()
                ));
            }
            let layout = &*(mapped as *const ArenaShmLayout);
            if layout.magic != WIRE_MAGIC {
                posix_ffi::munmap(mapped, size);
                posix_ffi::close(fd);
                return Err(format!("invalid SHM magic 0x{:08x}", layout.magic));
            }
            if layout.slot_count as usize != RING_SLOT_COUNT
                || layout.slot_capacity as usize != RING_SLOT_CAPACITY
            {
                posix_ffi::munmap(mapped, size);
                posix_ffi::close(fd);
                return Err("SHM ring dimensions mismatch".into());
            }

            Ok(Self {
                ptr: mapped as *mut ArenaShmLayout,
                fd,
                shm_name: shm_name.to_string(),
                cursor: 0,
            })
        }
    }

    #[cfg(not(unix))]
    pub fn open(_shm_name: &str) -> Result<Self, String> {
        Err("POSIX shm_open arena not supported on non-Unix platforms".into())
    }

    /// Poll for a READY slot, transition READY -> READING via AcqRel CAS,
    /// copy payload safely bounded by `RING_SLOT_CAPACITY`, and release READING -> FREE.
    #[cfg(unix)]
    pub fn poll_next_slot(&mut self) -> Option<AcquiredSnapshot> {
        let layout = unsafe { &*self.ptr };
        for i in 0..RING_SLOT_COUNT {
            let idx = (self.cursor + i) % RING_SLOT_COUNT;
            let slot = &layout.slots[idx];
            if slot
                .header
                .slot_state
                .compare_exchange(
                    SLOT_STATE_READY,
                    SLOT_STATE_READING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                self.cursor = (idx + 1) % RING_SLOT_COUNT;
                let seq = slot.header.sequence_number;
                let ts = slot.header.timestamp_nanos;
                let sig = LifecycleSignal::from_wire_u32(slot.header.lifecycle_signal)
                    .unwrap_or(LifecycleSignal::NormalFrame);
                let gap = slot.header.frame_gap_count;
                let sz = (slot.header.payload_size_bytes as usize).min(RING_SLOT_CAPACITY);
                let payload = slot.payload[..sz].to_vec();

                // Transition READING -> FREE with Release ordering
                slot.header
                    .slot_state
                    .store(SLOT_STATE_FREE, Ordering::Release);

                return Some(AcquiredSnapshot {
                    slot_index: idx,
                    sequence_number: seq,
                    timestamp_nanos: ts,
                    lifecycle_signal: sig,
                    frame_gap_count: gap,
                    payload,
                });
            }
        }
        None
    }

    #[cfg(not(unix))]
    pub fn poll_next_slot(&mut self) -> Option<AcquiredSnapshot> {
        None
    }
}

impl Drop for ShmArenaConsumer {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if !self.ptr.is_null() {
                posix_ffi::munmap(
                    self.ptr as *mut c_void,
                    std::mem::size_of::<ArenaShmLayout>(),
                );
                self.ptr = ptr::null_mut();
            }
            if self.fd >= 0 {
                posix_ffi::close(self.fd);
                self.fd = -1;
            }
        }
    }
}
