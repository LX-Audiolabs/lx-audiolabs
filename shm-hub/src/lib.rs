//! Cross-process / cross-DLL publish/subscribe hub backed by named shared memory.
//!
//! Each `.clap` / `.vst3` / `.dll` is a separate cdylib, so a process-global
//! `OnceLock` in a statically-linked rlib is NOT shared between plugins. Two
//! different plugin files each get their own copy and therefore cannot talk
//! through a plain global.
//!
//! Solution: a single named shared-memory segment (`CreateFileMapping` on
//! Windows, `shm_open`+`mmap` on macOS via the `shared_memory` crate). All
//! plugin instances in the host process map the SAME segment.
//!
//! Two registries live in the segment:
//!   * **Publisher slots** — each producer claims one and writes its payload
//!     plus a `target` name (which consumer to send to; empty = broadcast).
//!   * **Consumer slots** — each consumer claims one and publishes its instance
//!     `name`, so producers can list available targets. This is the
//!     bidirectional half: producers read consumer names, consumers read
//!     producer payloads.
//!
//! Concurrency: each slot is a seqlock. The single writer bumps `seq` to odd,
//! writes the payload via raw pointers, then bumps to even. Readers copy the
//! payload and retry if `seq` changed or was odd. Payload fields live in
//! `UnsafeCell`; byte access uses raw pointers (`copy_nonoverlapping`).
//!
//! Liveness: each write stamps `heartbeat_ms` (wall-clock millis). Readers drop
//! slots whose heartbeat is older than `STALE_MS`, so a removed plugin's entry
//! disappears on its own. Slots are claimed via CAS so two instances never share
//! one; a slot held by a dead instance (stale heartbeat) is reclaimable.

use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use shared_memory::{Shmem, ShmemConf};

/// Number of spectrum bins per payload frame.
pub const SPECTRUM_BINS: usize = 1024;
/// Maximum number of publisher slots.
pub const MAX_SLOTS: usize = 16;
/// Maximum number of consumer instances advertising a name.
pub const MAX_CONSUMERS: usize = 16;
/// Maximum label/name length in bytes (UTF-8).
pub const MAX_NAME_LEN: usize = 32;
/// A slot is considered dead if no heartbeat arrived within this window.
pub const STALE_MS: u64 = 500;

/// OS-global name for the segment. The `_vN` suffix is bumped whenever the
/// slot layout or claim protocol changes, so an old plugin (different layout)
/// maps a *separate* segment instead of colliding with the new one.
/// `_v2`: added the `claimed` flag for atomic auto-slot assignment.
/// `_v3`: publisher slots gained a `target` name; added the consumer-name registry.
/// `_v4`: publisher slots gained `band_energy` [f32; 5] for dynamic-EQ triggering.
const SHM_OS_ID: &str = "lxaudiolabs_lucent_relay_v4";
/// "LXRD" — marks a fully-initialized segment.
const MAGIC: u32 = 0x4C58_5244;
const VERSION: u32 = 4;
/// Number of EQ bands for band-energy reporting.
pub const EQ_BANDS: usize = 5;

/// Alias for backward compatibility.
pub const MAX_LUCENTS: usize = MAX_CONSUMERS;

/// One publisher's data. `#[repr(C)]` so the byte layout is identical across DLLs.
#[repr(C)]
struct PublisherSlot {
    /// Seqlock counter: even = stable, odd = write in progress.
    seq: AtomicU32,
    /// Auto-slot ownership: 0 = free, 1 = claimed. CAS-guarded; a slot whose
    /// owner died (stale heartbeat) can be reclaimed.
    claimed: AtomicU32,
    /// Wall-clock millis of the last write (liveness).
    heartbeat_ms: AtomicU64,
    /// Payload (seqlock-protected, accessed via raw pointers):
    name_len: UnsafeCell<u32>,
    active: UnsafeCell<u32>,
    name: UnsafeCell<[u8; MAX_NAME_LEN]>,
    /// Target consumer instance name; empty = broadcast to every consumer.
    target_len: UnsafeCell<u32>,
    target: UnsafeCell<[u8; MAX_NAME_LEN]>,
    bins: UnsafeCell<[f32; SPECTRUM_BINS]>,
    /// Per-band energy (dB) for dynamic-EQ triggering: Low Shelf, Peak 1–3, High Shelf.
    band_energy: UnsafeCell<[f32; EQ_BANDS]>,
}

// SAFETY: all cross-thread access goes through atomics (seq/heartbeat) and the
// seqlock-guarded raw-pointer payload; we never hand out `&` to the payload.
unsafe impl Sync for PublisherSlot {}

/// One consumer instance advertising its name so publishers can target it.
#[repr(C)]
struct ConsumerSlot {
    seq: AtomicU32,
    claimed: AtomicU32,
    heartbeat_ms: AtomicU64,
    name_len: UnsafeCell<u32>,
    name: UnsafeCell<[u8; MAX_NAME_LEN]>,
}

// SAFETY: see PublisherSlot.
unsafe impl Sync for ConsumerSlot {}

#[repr(C)]
struct HubShared {
    magic: AtomicU32,
    version: AtomicU32,
    slots: [PublisherSlot; MAX_SLOTS],
    consumers: [ConsumerSlot; MAX_CONSUMERS],
}

// Compile-time layout guarantees so the segment is byte-compatible everywhere.
const _: () = {
    assert!(core::mem::align_of::<PublisherSlot>() == 8);
    assert!(core::mem::align_of::<ConsumerSlot>() == 8);
    assert!(
        core::mem::size_of::<HubShared>()
            == 8 + MAX_SLOTS * core::mem::size_of::<PublisherSlot>()
                + MAX_CONSUMERS * core::mem::size_of::<ConsumerSlot>()
    );
};

/// Wall-clock time in milliseconds — consistent across all plugins in the
/// process, which is what makes the heartbeat comparison valid.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The display name a consumer advertises and is addressed by. Falls back to
/// "Hub N" (slot+1) when the user hasn't typed one, so unnamed instances are
/// still discoverable + targetable.
pub fn display_name(name: &str, slot: u8) -> String {
    if name.trim().is_empty() {
        format!("Hub {}", slot + 1)
    } else {
        name.to_string()
    }
}

/// Legacy alias for backward compatibility.
pub fn lucent_display_name(name: &str, slot: u8) -> String {
    display_name(name, slot)
}

/// Copy a UTF-8 name into a fixed slot buffer, returning the written length.
/// # Safety: `buf` must point to `MAX_NAME_LEN` writable bytes.
unsafe fn write_name_bytes(buf: *mut u8, name: &str) -> u32 {
    let bytes = name.as_bytes();
    let len = bytes.len().min(MAX_NAME_LEN);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
    for i in len..MAX_NAME_LEN {
        *buf.add(i) = 0;
    }
    len as u32
}

/// Handle to the shared relay segment. Held for the process lifetime inside a
/// `OnceLock` so the mapping is never unmapped early.
pub struct RelayHub {
    _shmem: Shmem,
    shared: *const HubShared,
}

// SAFETY: `shared` points into the shared mapping; all access is via atomics +
// seqlock-guarded raw pointers (see PublisherSlot). The mapping outlives the handle.
unsafe impl Send for RelayHub {}
unsafe impl Sync for RelayHub {}

/// Generic CAS-based slot claim shared by both registries.
/// `claimed`/`heartbeat` are the slot's atomics. Returns whether the claim won.
fn try_claim(claimed: &AtomicU32, heartbeat: &AtomicU64, now_ms: u64) -> bool {
    let mut c = claimed.load(Ordering::Acquire);
    if c == 1 {
        let hb = heartbeat.load(Ordering::Acquire);
        let stale = hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS;
        if stale {
            let _ = claimed.compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed);
            c = claimed.load(Ordering::Acquire);
        }
    }
    if c == 0
        && claimed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        heartbeat.store(now_ms, Ordering::Release);
        true
    } else {
        false
    }
}

impl RelayHub {
    fn open_or_create() -> Option<RelayHub> {
        let size = core::mem::size_of::<HubShared>();

        let (shmem, is_creator) = match ShmemConf::new().os_id(SHM_OS_ID).size(size).create() {
            Ok(m) => (m, true),
            Err(_) => (ShmemConf::new().os_id(SHM_OS_ID).open().ok()?, false),
        };

        let shared = shmem.as_ptr() as *const HubShared;

        if is_creator {
            unsafe {
                (*shared).version.store(VERSION, Ordering::Release);
                (*shared).magic.store(MAGIC, Ordering::Release);
            }
        } else {
            let mut spins = 0u32;
            while unsafe { (*shared).magic.load(Ordering::Acquire) } != MAGIC {
                std::thread::yield_now();
                spins += 1;
                if spins > 1_000_000 {
                    return None;
                }
            }
        }

        Some(RelayHub { _shmem: shmem, shared })
    }

    // ---- Publisher registry (writer = producer, reader = consumer) -----------

    /// Atomically claim a free publisher slot at startup.
    pub fn claim_slot(&self, now_ms: u64) -> Option<u8> {
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };
            if try_claim(&s.claimed, &s.heartbeat_ms, now_ms) {
                return Some(idx as u8);
            }
        }
        None
    }

    /// Release a previously claimed publisher slot on teardown.
    pub fn release_slot(&self, slot: u8) {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        s.heartbeat_ms.store(0, Ordering::Release);
        s.claimed.store(0, Ordering::Release);
    }

    /// Publish this slot's payload + label + target + band energy.
    /// `target` empty = broadcast to every consumer; otherwise only the consumer
    /// whose instance name equals `target` receives the payload.
    pub fn write(&self, slot: u8, label: &str, target: &str, bins: &[f32], band_energy: &[f32], now_ms: u64) {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return;
        }
        let s = unsafe { &(*self.shared).slots[idx] };

        let seq0 = s.seq.load(Ordering::Relaxed);
        s.seq.store(seq0.wrapping_add(1), Ordering::Release);
        fence(Ordering::Release);

        unsafe {
            *s.active.get() = 1;
            *s.name_len.get() = write_name_bytes(s.name.get() as *mut u8, label);
            *s.target_len.get() = write_name_bytes(s.target.get() as *mut u8, target);

            let bins_ptr = s.bins.get() as *mut f32;
            let n = bins.len().min(SPECTRUM_BINS);
            std::ptr::copy_nonoverlapping(bins.as_ptr(), bins_ptr, n);
            for i in n..SPECTRUM_BINS {
                *bins_ptr.add(i) = -90.0;
            }

            let be_ptr = s.band_energy.get() as *mut f32;
            let m = band_energy.len().min(EQ_BANDS);
            std::ptr::copy_nonoverlapping(band_energy.as_ptr(), be_ptr, m);
            for i in m..EQ_BANDS {
                *be_ptr.add(i) = -90.0;
            }
        }

        fence(Ordering::Release);
        s.seq.store(seq0.wrapping_add(2), Ordering::Release);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
    }

    /// Presence heartbeat: refresh label/target/heartbeat WITHOUT touching
    /// `bins` or `band_energy`, so it can keep a publisher live while transport
    /// is stopped without overwriting the audio thread's data.
    pub fn touch(&self, slot: u8, label: &str, target: &str, now_ms: u64) {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return;
        }
        let s = unsafe { &(*self.shared).slots[idx] };

        let seq0 = s.seq.load(Ordering::Relaxed);
        s.seq.store(seq0.wrapping_add(1), Ordering::Release);
        fence(Ordering::Release);

        unsafe {
            *s.active.get() = 1;
            *s.name_len.get() = write_name_bytes(s.name.get() as *mut u8, label);
            *s.target_len.get() = write_name_bytes(s.target.get() as *mut u8, target);
        }

        fence(Ordering::Release);
        s.seq.store(seq0.wrapping_add(2), Ordering::Release);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
    }

    /// Collect all live, active publisher slots whose target is empty (broadcast)
    /// or equals `my_name`. Stale slots are skipped.
    pub fn read_active(&self, my_name: &str, now_ms: u64) -> Vec<(String, Vec<f32>)> {
        let mut out = Vec::new();
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..4 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }

                let mut name_buf = [0u8; MAX_NAME_LEN];
                let mut target_buf = [0u8; MAX_NAME_LEN];
                let mut bins = vec![0.0f32; SPECTRUM_BINS];
                let (active, name_len, target_len) = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.bins.get() as *const f32,
                        bins.as_mut_ptr(),
                        SPECTRUM_BINS,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.name.get() as *const u8,
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.target.get() as *const u8,
                        target_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (
                        *s.active.get(),
                        (*s.name_len.get() as usize).min(MAX_NAME_LEN),
                        (*s.target_len.get() as usize).min(MAX_NAME_LEN),
                    )
                };

                fence(Ordering::Acquire);
                let seq2 = s.seq.load(Ordering::Acquire);
                if seq1 == seq2 {
                    if active != 0 {
                        let target = String::from_utf8_lossy(&target_buf[..target_len]);
                        if target.is_empty() || target == my_name {
                            let name =
                                String::from_utf8_lossy(&name_buf[..name_len]).into_owned();
                            out.push((name, bins));
                        }
                    }
                    break;
                }
            }
        }
        out
    }

    /// Read band energy from a specific publisher slot (by index). Returns None
    /// if the slot is stale or the seqlock fails.
    pub fn read_band_energy(&self, slot: u8, now_ms: u64) -> Option<[f32; EQ_BANDS]> {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return None;
        }
        let s = unsafe { &(*self.shared).slots[idx] };

        let hb = s.heartbeat_ms.load(Ordering::Acquire);
        if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
            return None;
        }

        for _ in 0..4 {
            let seq1 = s.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                continue;
            }
            let mut energy = [0.0f32; EQ_BANDS];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    s.band_energy.get() as *const f32,
                    energy.as_mut_ptr(),
                    EQ_BANDS,
                );
            }
            fence(Ordering::Acquire);
            let seq2 = s.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return Some(energy);
            }
        }
        None
    }

    /// Find a live publisher slot by name and return its band energy + slot index.
    pub fn find_band_energy(&self, name: &str, now_ms: u64) -> Option<(u8, [f32; EQ_BANDS])> {
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..4 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                let mut name_buf = [0u8; MAX_NAME_LEN];
                let name_len = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.name.get() as *const u8,
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };
                fence(Ordering::Acquire);
                if seq1 == s.seq.load(Ordering::Acquire) {
                    let slot_name = String::from_utf8_lossy(&name_buf[..name_len]);
                    if slot_name == name {
                        return self.read_band_energy(idx as u8, now_ms)
                            .map(|e| (idx as u8, e));
                    }
                }
                break;
            }
        }
        None
    }

    // ---- Consumer registry (writer = consumer, reader = publisher) -----------

    /// Claim a free consumer-name slot at startup.
    pub fn claim_lucent_slot(&self, now_ms: u64) -> Option<u8> {
        for idx in 0..MAX_CONSUMERS {
            let s = unsafe { &(*self.shared).consumers[idx] };
            if try_claim(&s.claimed, &s.heartbeat_ms, now_ms) {
                return Some(idx as u8);
            }
        }
        None
    }

    /// Release a previously claimed consumer-name slot on teardown.
    pub fn release_lucent_slot(&self, slot: u8) {
        let idx = slot as usize;
        if idx >= MAX_CONSUMERS {
            return;
        }
        let s = unsafe { &(*self.shared).consumers[idx] };
        s.heartbeat_ms.store(0, Ordering::Release);
        s.claimed.store(0, Ordering::Release);
    }

    /// Publish this consumer instance's name + heartbeat.
    pub fn write_lucent_name(&self, slot: u8, name: &str, now_ms: u64) {
        let idx = slot as usize;
        if idx >= MAX_CONSUMERS {
            return;
        }
        let s = unsafe { &(*self.shared).consumers[idx] };

        let seq0 = s.seq.load(Ordering::Relaxed);
        s.seq.store(seq0.wrapping_add(1), Ordering::Release);
        fence(Ordering::Release);
        unsafe {
            *s.name_len.get() = write_name_bytes(s.name.get() as *mut u8, name);
        }
        fence(Ordering::Release);
        s.seq.store(seq0.wrapping_add(2), Ordering::Release);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
    }

    /// Read one consumer slot's name if it is live and non-empty.
    /// Returns the name length. Allocation-free — safe on the audio thread.
    fn read_consumer_slot(&self, idx: usize, now_ms: u64, out: &mut [u8; MAX_NAME_LEN]) -> Option<usize> {
        let s = unsafe { &(*self.shared).consumers[idx] };
        let hb = s.heartbeat_ms.load(Ordering::Acquire);
        if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
            return None;
        }
        for _ in 0..4 {
            let seq1 = s.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                continue;
            }
            let name_len = unsafe {
                std::ptr::copy_nonoverlapping(s.name.get() as *const u8, out.as_mut_ptr(), MAX_NAME_LEN);
                (*s.name_len.get() as usize).min(MAX_NAME_LEN)
            };
            fence(Ordering::Acquire);
            if seq1 == s.seq.load(Ordering::Acquire) {
                return if name_len == 0 { None } else { Some(name_len) };
            }
        }
        None
    }

    /// True if a live consumer instance currently advertises exactly `name`.
    /// Audio-thread safe — no allocation.
    pub fn lucent_exists(&self, name: &str, now_ms: u64) -> bool {
        let mut buf = [0u8; MAX_NAME_LEN];
        for idx in 0..MAX_CONSUMERS {
            if let Some(n) = self.read_consumer_slot(idx, now_ms, &mut buf) {
                if &buf[..n] == name.as_bytes() {
                    return true;
                }
            }
        }
        false
    }

    /// If exactly one live consumer exists, copy its name and return the length;
    /// else `None`. Audio-thread safe — no allocation.
    pub fn single_lucent_name(&self, now_ms: u64, out: &mut [u8; MAX_NAME_LEN]) -> Option<usize> {
        let mut found: Option<usize> = None;
        let mut scratch = [0u8; MAX_NAME_LEN];
        for idx in 0..MAX_CONSUMERS {
            if let Some(n) = self.read_consumer_slot(idx, now_ms, &mut scratch) {
                if found.is_some() {
                    return None;
                }
                out[..n].copy_from_slice(&scratch[..n]);
                found = Some(n);
            }
        }
        found
    }

    /// List the names of all live consumer instances (for target dropdowns).
    /// Skips stale and empty-named slots; de-duplicates names.
    pub fn read_lucents(&self, now_ms: u64) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for idx in 0..MAX_CONSUMERS {
            let s = unsafe { &(*self.shared).consumers[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..4 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                let mut name_buf = [0u8; MAX_NAME_LEN];
                let name_len = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.name.get() as *const u8,
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };
                fence(Ordering::Acquire);
                let seq2 = s.seq.load(Ordering::Acquire);
                if seq1 == seq2 {
                    let name = String::from_utf8_lossy(&name_buf[..name_len]).into_owned();
                    if !name.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                    break;
                }
            }
        }
        out
    }
}

/// Process-global hub backed by shared memory. Returns `None` if the segment
/// could not be mapped (callers then behave as if no publishers exist).
pub fn relay_hub() -> Option<&'static RelayHub> {
    static HUB: OnceLock<Option<RelayHub>> = OnceLock::new();
    HUB.get_or_init(RelayHub::open_or_create).as_ref()
}
