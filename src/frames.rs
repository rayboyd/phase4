//! The shared memory frame region of the `Phase4Engine` link contract, layout
//! version 1.
//!
//! A frame region is anonymous `MAP_SHARED` memory that the XPC service hands
//! to its client with `xpc_shmem_create`. The frame writer publishes every
//! analysis snapshot into it under a sequence lock, and a reader in either
//! process copies the latest frame without taking a lock. Every header and
//! payload word is read and written atomically, so the lock never relies on a
//! data race.

use crate::dsp::{ChannelLevel, BAND_COUNT};
use std::ffi::c_void;
use std::io;
use std::ptr::{self, NonNull};
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// The ASCII bytes at the start of every frame region.
pub const FRAME_MAGIC: [u8; 4] = *b"P4FR";

/// The frame region layout version.
pub const FRAME_LAYOUT_VERSION: u32 = 1;

/// Bytes before the first channel record.
pub const FRAME_HEADER_BYTES: usize = 64;

/// Bytes in one 32-bit word of the region.
const WORD_BYTES: usize = 4;

/// Bytes in one channel record, the peak followed by every bin.
pub const CHANNEL_RECORD_BYTES: usize = (1 + BAND_COUNT) * WORD_BYTES;

/// Header field offsets in bytes.
pub const OFFSET_MAGIC: usize = 0;
pub const OFFSET_LAYOUT_VERSION: usize = 4;
pub const OFFSET_SEQUENCE: usize = 8;
pub const OFFSET_PUBLISHED_NS: usize = 16;
pub const OFFSET_CHANNEL_COUNT: usize = 24;
pub const OFFSET_BAND_COUNT: usize = 28;
pub const OFFSET_SAMPLE_RATE: usize = 32;
pub const OFFSET_MIDI_FLAGS: usize = 36;
pub const OFFSET_MIDI_STEPS: usize = 40;
pub const OFFSET_MIDI_TRANSPORT_COUNT: usize = 44;
pub const OFFSET_MIDI_TRANSPORT_LAST: usize = 48;

/// The peak's offset within a channel record.
const RECORD_PEAK_OFFSET: usize = 0;

/// The first bin's offset within a channel record.
const RECORD_BINS_OFFSET: usize = WORD_BYTES;

/// The band count as the header stores it. 32 always fits in u32.
const HEADER_BAND_COUNT: u32 = BAND_COUNT as u32;

/// Bit 0 of `midi_flags`, set when MIDI input is configured.
pub const MIDI_FLAG_CONFIGURED: u32 = 1;

/// Read attempts a reader makes before it keeps its previous frame.
pub const READ_ATTEMPTS: usize = 3;

extern "C" {
    fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
}

/// The MIDI state carried by one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameMidi {
    /// MIDI 1/16 note steps since the last Start, wrapping at 2^32.
    pub steps: u32,

    /// Start, Stop and Continue events received, wrapping at 2^32.
    pub transport_count: u32,

    /// The most recent transport event, 0 none, 1 Start, 2 Stop, 3 Continue.
    pub transport_last: u32,
}

/// The fields a region's creator writes once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub layout_version: u32,
    pub channel_count: u32,
    pub band_count: u32,
    pub sample_rate: u32,
    pub midi_flags: u32,
}

/// One frame copied out of a region by the sequence lock reader.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub sequence: u64,
    pub published_ns: u64,
    pub midi: FrameMidi,
    pub channels: Vec<ChannelLevel>,
}

/// Bytes the layout uses for `channel_count` channels.
#[must_use]
pub const fn frame_bytes(channel_count: usize) -> usize {
    FRAME_HEADER_BYTES + channel_count * CHANNEL_RECORD_BYTES
}

/// The current `CLOCK_UPTIME_RAW` time in nanoseconds.
#[must_use]
pub fn uptime_raw_ns() -> u64 {
    // SAFETY: clock_gettime_nsec_np takes a clock identifier by value and
    // touches no memory of ours. CLOCK_UPTIME_RAW is always available.
    unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) }
}

/// The 32-bit word `offset` bytes into the mapping at `base`.
///
/// # Safety
///
/// `base + offset` must lie inside a live mapping, aligned to 4 bytes, for as
/// long as the returned reference is used.
// The caller guarantees the alignment the cast needs.
#[allow(clippy::cast_ptr_alignment)]
unsafe fn word32<'a>(base: *const u8, offset: usize) -> &'a AtomicU32 {
    AtomicU32::from_ptr(base.add(offset).cast_mut().cast::<u32>())
}

/// The 64-bit word `offset` bytes into the mapping at `base`.
///
/// # Safety
///
/// `base + offset` must lie inside a live mapping, aligned to 8 bytes, for as
/// long as the returned reference is used.
// The caller guarantees the alignment the cast needs.
#[allow(clippy::cast_ptr_alignment)]
unsafe fn word64<'a>(base: *const u8, offset: usize) -> &'a AtomicU64 {
    AtomicU64::from_ptr(base.add(offset).cast_mut().cast::<u64>())
}

/// The offset of channel `index`'s record.
const fn record_offset(index: usize) -> usize {
    FRAME_HEADER_BYTES + index * CHANNEL_RECORD_BYTES
}

/// The system page size in bytes.
fn page_size() -> io::Result<usize> {
    // SAFETY: sysconf reads a system constant and touches no memory of ours.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size).map_err(|_| io::Error::last_os_error())
}

/// A frame region in page aligned anonymous `MAP_SHARED` memory, which
/// `xpc_shmem_create` accepts. One writer publishes into it, and it is
/// unmapped on drop.
#[derive(Debug)]
pub struct FrameRegion {
    base: NonNull<u8>,
    mapped_len: usize,
    channel_count: usize,
}

// SAFETY: the region is plain shared memory that is only accessed through
// atomics, and the mapping lives until the region is dropped.
unsafe impl Send for FrameRegion {}
// SAFETY: as above, every access through a shared reference is atomic.
unsafe impl Sync for FrameRegion {}

impl FrameRegion {
    /// Maps a zeroed region of `frame_bytes(channel_count)` rounded up to a
    /// whole number of pages, then writes the fields the contract marks Once.
    /// `sequence` starts at zero.
    ///
    /// # Errors
    ///
    /// Returns the OS error when `mmap` fails, or an invalid input error when
    /// the channel count does not fit the header.
    pub fn new(channel_count: usize, sample_rate: u32, midi_configured: bool) -> io::Result<Self> {
        let header_channel_count = u32::try_from(channel_count).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "The channel count does not fit the frame header",
            )
        })?;
        let page = page_size()?;
        let mapped_len = frame_bytes(channel_count).div_ceil(page) * page;

        // SAFETY: an anonymous shared mapping with no address hint, checked
        // for failure before use.
        let pointer = unsafe {
            libc::mmap(
                ptr::null_mut(),
                mapped_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let base = NonNull::new(pointer.cast::<u8>())
            .ok_or_else(|| io::Error::other("mmap returned a null mapping"))?;

        let region = Self {
            base,
            mapped_len,
            channel_count,
        };
        let midi_flags = if midi_configured {
            MIDI_FLAG_CONFIGURED
        } else {
            0
        };

        // SAFETY: every offset lies inside the header of this live mapping and
        // is aligned to 4 bytes. Nothing else can see the region yet.
        unsafe {
            let base = region.base.as_ptr();
            word32(base, OFFSET_MAGIC).store(u32::from_le_bytes(FRAME_MAGIC), Ordering::Relaxed);
            word32(base, OFFSET_LAYOUT_VERSION).store(FRAME_LAYOUT_VERSION, Ordering::Relaxed);
            word32(base, OFFSET_CHANNEL_COUNT).store(header_channel_count, Ordering::Relaxed);
            word32(base, OFFSET_BAND_COUNT).store(HEADER_BAND_COUNT, Ordering::Relaxed);
            word32(base, OFFSET_SAMPLE_RATE).store(sample_rate, Ordering::Relaxed);
            word32(base, OFFSET_MIDI_FLAGS).store(midi_flags, Ordering::Relaxed);
        }

        Ok(region)
    }

    /// Bytes the layout uses.
    #[must_use]
    pub fn frame_bytes(&self) -> usize {
        frame_bytes(self.channel_count)
    }

    /// Bytes mapped, a whole number of pages.
    #[must_use]
    pub fn mapped_len(&self) -> usize {
        self.mapped_len
    }

    /// The start of the mapping, for `xpc_shmem_create`.
    #[must_use]
    pub fn as_mut_ptr(&self) -> *mut c_void {
        self.base.as_ptr().cast::<c_void>()
    }

    /// Publishes one frame with the sequence lock. Returns `false` and
    /// publishes nothing when any peak or bin is not finite. Only the frame
    /// writer thread calls this.
    ///
    /// # Panics
    ///
    /// Panics if `channels.len()` differs from the region's channel count.
    #[must_use]
    pub fn publish(&self, channels: &[ChannelLevel], midi: FrameMidi, published_ns: u64) -> bool {
        assert_eq!(
            channels.len(),
            self.channel_count,
            "a frame must carry exactly the region's channel count"
        );
        let finite = channels.iter().all(|channel| {
            channel.peak.is_finite() && channel.bins.iter().all(|bin| bin.is_finite())
        });
        if !finite {
            return false;
        }

        // SAFETY: every offset lies inside `frame_bytes`, which the mapping
        // covers, and each is aligned to its word size.
        unsafe {
            let base = self.base.as_ptr();
            let sequence = word64(base, OFFSET_SEQUENCE);
            let stable = sequence.load(Ordering::Relaxed);

            sequence.store(stable.wrapping_add(1), Ordering::Relaxed);
            fence(Ordering::Release);

            word64(base, OFFSET_PUBLISHED_NS).store(published_ns, Ordering::Relaxed);
            word32(base, OFFSET_MIDI_STEPS).store(midi.steps, Ordering::Relaxed);
            word32(base, OFFSET_MIDI_TRANSPORT_COUNT)
                .store(midi.transport_count, Ordering::Relaxed);
            word32(base, OFFSET_MIDI_TRANSPORT_LAST).store(midi.transport_last, Ordering::Relaxed);
            for (index, channel) in channels.iter().enumerate() {
                let record = record_offset(index);
                word32(base, record + RECORD_PEAK_OFFSET)
                    .store(channel.peak.to_bits(), Ordering::Relaxed);
                for (band, bin) in channel.bins.iter().enumerate() {
                    word32(base, record + RECORD_BINS_OFFSET + band * WORD_BYTES)
                        .store(bin.to_bits(), Ordering::Relaxed);
                }
            }

            sequence.store(stable.wrapping_add(2), Ordering::Release);
        }
        true
    }

    /// Reads the latest frame with the sequence lock.
    #[must_use]
    pub fn read(&self) -> Option<Frame> {
        // SAFETY: the region's own mapping is live, page aligned and at least
        // `mapped_len` bytes long.
        unsafe { read_frame(self.base.as_ptr(), self.mapped_len) }
    }
}

impl Drop for FrameRegion {
    fn drop(&mut self) {
        // SAFETY: the mapping was created by `new` with this address and
        // length and is unmapped exactly once, here.
        let result = unsafe { libc::munmap(self.as_mut_ptr(), self.mapped_len) };
        if result != 0 {
            log::warn!(
                "Failed to unmap the frame region: {}",
                io::Error::last_os_error()
            );
        }
    }
}

/// Reads the fields written once. Returns `None` when `len` is shorter than
/// the header or the magic does not match.
///
/// # Safety
///
/// `base` must point to at least `len` readable bytes, aligned to 8 bytes,
/// that stay mapped for the call.
#[must_use]
pub unsafe fn read_header(base: *const u8, len: usize) -> Option<FrameHeader> {
    if len < FRAME_HEADER_BYTES {
        return None;
    }
    if word32(base, OFFSET_MAGIC).load(Ordering::Relaxed) != u32::from_le_bytes(FRAME_MAGIC) {
        return None;
    }
    Some(FrameHeader {
        layout_version: word32(base, OFFSET_LAYOUT_VERSION).load(Ordering::Relaxed),
        channel_count: word32(base, OFFSET_CHANNEL_COUNT).load(Ordering::Relaxed),
        band_count: word32(base, OFFSET_BAND_COUNT).load(Ordering::Relaxed),
        sample_rate: word32(base, OFFSET_SAMPLE_RATE).load(Ordering::Relaxed),
        midi_flags: word32(base, OFFSET_MIDI_FLAGS).load(Ordering::Relaxed),
    })
}

/// Reads the latest frame with the sequence lock. Returns `None` when the
/// header is invalid, `len` is too short for its channel count, no frame has
/// been published, or every one of `READ_ATTEMPTS` attempts overlapped a write.
///
/// # Safety
///
/// `base` must point to a frame region of at least `len` readable bytes,
/// aligned to 8 bytes, that stays mapped for the call.
#[must_use]
pub unsafe fn read_frame(base: *const u8, len: usize) -> Option<Frame> {
    let header = read_header(base, len)?;
    if header.layout_version != FRAME_LAYOUT_VERSION || header.band_count as usize != BAND_COUNT {
        return None;
    }
    let channel_count = header.channel_count as usize;
    if len < frame_bytes(channel_count) {
        return None;
    }

    let sequence = word64(base, OFFSET_SEQUENCE);
    let mut channels = vec![ChannelLevel::default(); channel_count];
    for _ in 0..READ_ATTEMPTS {
        let before = sequence.load(Ordering::Acquire);
        if before == 0 {
            return None;
        }
        if !before.is_multiple_of(2) {
            continue;
        }

        let published_ns = word64(base, OFFSET_PUBLISHED_NS).load(Ordering::Relaxed);
        let midi = FrameMidi {
            steps: word32(base, OFFSET_MIDI_STEPS).load(Ordering::Relaxed),
            transport_count: word32(base, OFFSET_MIDI_TRANSPORT_COUNT).load(Ordering::Relaxed),
            transport_last: word32(base, OFFSET_MIDI_TRANSPORT_LAST).load(Ordering::Relaxed),
        };
        for (index, channel) in channels.iter_mut().enumerate() {
            let record = record_offset(index);
            channel.peak =
                f32::from_bits(word32(base, record + RECORD_PEAK_OFFSET).load(Ordering::Relaxed));
            for (band, bin) in channel.bins.iter_mut().enumerate() {
                *bin = f32::from_bits(
                    word32(base, record + RECORD_BINS_OFFSET + band * WORD_BYTES)
                        .load(Ordering::Relaxed),
                );
            }
        }

        fence(Ordering::Acquire);
        if sequence.load(Ordering::Relaxed) == before {
            return Some(Frame {
                sequence: before,
                published_ns,
                midi,
                channels,
            });
        }
    }
    None
}

/// Carries a frame region from bootstrap, which creates it once the channel
/// count and sample rate are known, to the owner that shares it.
#[derive(Debug, Clone, Default)]
pub struct FrameRegionSlot(Arc<OnceLock<Arc<FrameRegion>>>);

impl PartialEq for FrameRegionSlot {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl FrameRegionSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The region bootstrap created, or `None` before bootstrap has run.
    #[must_use]
    pub fn region(&self) -> Option<Arc<FrameRegion>> {
        self.0.get().cloned()
    }

    /// Stores the region. Returns `false` and leaves the first region in
    /// place when the slot is already filled.
    pub(crate) fn fill(&self, region: Arc<FrameRegion>) -> bool {
        self.0.set(region).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::thread;

    const TEST_SAMPLE_RATE: u32 = 48_000;
    const STEREO: usize = 2;
    const STRESS_FRAMES: u32 = 10_000;

    fn level(value: f32) -> ChannelLevel {
        ChannelLevel {
            peak: value,
            bins: [value; BAND_COUNT],
        }
    }

    fn midi(value: u32) -> FrameMidi {
        FrameMidi {
            steps: value,
            transport_count: value,
            transport_last: value,
        }
    }

    #[test]
    fn layout_constants_match_the_contract() {
        assert_eq!(FRAME_HEADER_BYTES, 64);
        assert_eq!(CHANNEL_RECORD_BYTES, 132);
        assert_eq!(frame_bytes(STEREO), 328);
        assert_eq!(
            [
                OFFSET_MAGIC,
                OFFSET_LAYOUT_VERSION,
                OFFSET_SEQUENCE,
                OFFSET_PUBLISHED_NS,
                OFFSET_CHANNEL_COUNT,
                OFFSET_BAND_COUNT,
                OFFSET_SAMPLE_RATE,
                OFFSET_MIDI_FLAGS,
                OFFSET_MIDI_STEPS,
                OFFSET_MIDI_TRANSPORT_COUNT,
                OFFSET_MIDI_TRANSPORT_LAST,
            ],
            [0, 4, 8, 16, 24, 28, 32, 36, 40, 44, 48]
        );
    }

    #[test]
    fn a_new_region_holds_its_header_and_no_frame() {
        let region = FrameRegion::new(STEREO, TEST_SAMPLE_RATE, true).expect("the region maps");
        let page = page_size().expect("the page size is known");

        assert_eq!(region.mapped_len() % page, 0);
        assert!(region.mapped_len() >= region.frame_bytes());
        assert_eq!(region.frame_bytes(), frame_bytes(STEREO));

        // SAFETY: the region's own live mapping.
        let header = unsafe { read_header(region.base.as_ptr(), region.mapped_len()) };
        assert_eq!(
            header,
            Some(FrameHeader {
                layout_version: FRAME_LAYOUT_VERSION,
                channel_count: 2,
                band_count: 32,
                sample_rate: TEST_SAMPLE_RATE,
                midi_flags: MIDI_FLAG_CONFIGURED,
            })
        );
        // SAFETY: the region's own live mapping.
        let magic = unsafe { std::slice::from_raw_parts(region.base.as_ptr(), FRAME_MAGIC.len()) };
        assert_eq!(magic, FRAME_MAGIC);
        assert_eq!(region.read(), None);
    }

    #[test]
    fn a_region_without_midi_clears_the_midi_flag() {
        let region = FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps");
        // SAFETY: the region's own live mapping.
        let header = unsafe { read_header(region.base.as_ptr(), region.mapped_len()) }
            .expect("the header is valid");
        assert_eq!(header.midi_flags, 0);
    }

    #[test]
    fn publishing_advances_the_sequence_by_two() {
        let region = FrameRegion::new(STEREO, TEST_SAMPLE_RATE, false).expect("the region maps");
        let channels = [level(0.5), level(0.25)];

        assert!(region.publish(&channels, FrameMidi::default(), 1));
        assert_eq!(region.read().map(|frame| frame.sequence), Some(2));
        assert!(region.publish(&channels, FrameMidi::default(), 2));
        assert_eq!(region.read().map(|frame| frame.sequence), Some(4));
    }

    #[test]
    fn a_published_frame_reads_back() {
        let region = FrameRegion::new(STEREO, TEST_SAMPLE_RATE, true).expect("the region maps");
        let mut first = level(0.75);
        first.bins[BAND_COUNT - 1] = 3.5;
        let channels = [first, level(0.125)];
        let frame_midi = FrameMidi {
            steps: 12,
            transport_count: 3,
            transport_last: 1,
        };

        assert!(region.publish(&channels, frame_midi, 42));

        assert_eq!(
            region.read(),
            Some(Frame {
                sequence: 2,
                published_ns: 42,
                midi: frame_midi,
                channels: channels.to_vec(),
            })
        );
    }

    #[test]
    fn a_non_finite_peak_is_not_published() {
        let region = FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps");
        assert!(!region.publish(&[level(f32::NAN)], FrameMidi::default(), 1));
        assert_eq!(region.read(), None);
    }

    #[test]
    fn a_non_finite_bin_is_not_published() {
        let region = FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps");
        assert!(region.publish(&[level(0.5)], FrameMidi::default(), 1));

        let mut channel = level(0.5);
        channel.bins[3] = f32::INFINITY;
        assert!(!region.publish(&[channel], FrameMidi::default(), 2));
        assert_eq!(region.read().map(|frame| frame.sequence), Some(2));
    }

    #[test]
    fn a_reader_gives_up_while_the_sequence_is_odd() {
        let region = FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps");
        assert!(region.publish(&[level(0.5)], FrameMidi::default(), 1));

        // SAFETY: the region's own live mapping, aligned for the sequence.
        unsafe { word64(region.base.as_ptr(), OFFSET_SEQUENCE) }.store(3, Ordering::Release);

        assert_eq!(region.read(), None);
    }

    #[test]
    fn read_header_rejects_a_wrong_magic_and_a_short_length() {
        let region = FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps");
        let base = region.base.as_ptr();

        // SAFETY: the region's own live mapping.
        unsafe {
            assert_eq!(read_header(base, FRAME_HEADER_BYTES - 1), None);
            word32(base, OFFSET_MAGIC).store(0, Ordering::Relaxed);
            assert_eq!(read_header(base, region.mapped_len()), None);
        }
    }

    #[test]
    fn a_concurrent_reader_only_sees_whole_frames() {
        let region =
            Arc::new(FrameRegion::new(STEREO, TEST_SAMPLE_RATE, true).expect("the region maps"));
        let writing = Arc::new(AtomicBool::new(true));

        let writer_region = Arc::clone(&region);
        let writer_flag = Arc::clone(&writing);
        let writer = thread::spawn(move || {
            for number in 1..=STRESS_FRAMES {
                let value = number as f32;
                assert!(writer_region.publish(
                    &[level(value), level(value)],
                    midi(number),
                    u64::from(number)
                ));
            }
            writer_flag.store(false, Ordering::Release);
        });

        let mut frames_read = 0_u32;
        while writing.load(Ordering::Acquire) {
            if let Some(frame) = region.read() {
                let number = frame.midi.steps;
                let value = number as f32;
                assert_eq!(frame.published_ns, u64::from(number));
                assert_eq!(frame.midi, midi(number));
                for channel in &frame.channels {
                    assert_eq!(channel.peak.to_bits(), value.to_bits());
                    assert!(channel
                        .bins
                        .iter()
                        .all(|bin| bin.to_bits() == value.to_bits()));
                }
                frames_read += 1;
            }
        }
        writer.join().expect("the writer finishes");

        assert!(frames_read > 0);
        assert_eq!(
            region.read().map(|frame| frame.sequence),
            Some(u64::from(STRESS_FRAMES) * 2)
        );
    }

    #[test]
    fn a_slot_keeps_the_first_region() {
        let slot = FrameRegionSlot::new();
        assert!(slot.region().is_none());

        let first =
            Arc::new(FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps"));
        let second =
            Arc::new(FrameRegion::new(1, TEST_SAMPLE_RATE, false).expect("the region maps"));

        assert!(slot.fill(Arc::clone(&first)));
        assert!(!slot.fill(second));
        assert!(slot
            .region()
            .is_some_and(|region| Arc::ptr_eq(&region, &first)));
        assert_eq!(slot.clone(), slot);
        assert_ne!(FrameRegionSlot::new(), slot);
    }
}
