//! Null-capture PCM source math.
//!
//! virtio-drivers 0.13 exposes input-stream enumeration on `VirtIOSound`
//! but its PCM transfer path is TX-only (`pcm_xfer`/`pcm_xfer_nb` push
//! descriptors to the transmit queue), so capture runs as an honest null
//! source: frames are synthesized digital silence paced against the
//! 100 Hz monotonic clock, with per-frame timestamps and the same
//! s16-stereo FNV-1a checksum unit the playback sink uses. When the
//! crate grows an RX path this module becomes the pacing/checksum layer
//! in front of real device frames.

use crate::audio::AudioSampleFormat;
use crate::audio_pcm::{pcm_pack_words, CHECKSUM_SEED};

/// Kernel monotonic tick rate (see the timer source brought up by the
/// kernel): 100 ticks per second.
pub const CAPTURE_TICK_HZ: u64 = 100;

/// Frames the capture source owes by `now_tick` given everything already
/// produced since `start_tick`. Real-time pacing bound: a reader can
/// never pull more frames than wall-clock time justifies.
pub fn capture_frames_due(
    frames_produced: u64,
    start_tick: u64,
    now_tick: u64,
    rate_hz: u32,
) -> u64 {
    if now_tick <= start_tick || rate_hz == 0 {
        return 0;
    }
    let target =
        (now_tick - start_tick) as u128 * rate_hz as u128 / CAPTURE_TICK_HZ as u128;
    target.saturating_sub(frames_produced as u128) as u64
}

/// Monotonic tick of `frame_index` counting from `start_tick` at
/// `rate_hz` (floor division; frame 0 carries the start tick).
pub fn capture_frame_tick(start_tick: u64, frame_index: u64, rate_hz: u32) -> u64 {
    if rate_hz == 0 {
        return start_tick;
    }
    start_tick + (frame_index as u128 * CAPTURE_TICK_HZ as u128 / rate_hz as u128) as u64
}

/// Pack `frame_count` frames of digital silence (zero amplitude in the
/// stream format) into interleaved IPC words. Returns the word count.
pub fn capture_pack_silence(
    format: AudioSampleFormat,
    channels: u32,
    frame_count: usize,
    words: &mut [u64],
) -> usize {
    let sample_count = frame_count.saturating_mul(channels as usize);
    let zeros = [0.0f32; 128];
    let mut packed = 0usize;
    let mut remaining = sample_count;
    while remaining > 0 && packed < words.len() {
        let take = remaining.min(zeros.len());
        packed += pcm_pack_words(format, &zeros[..take], &mut words[packed..]);
        remaining -= take;
    }
    packed
}

/// Fold `frame_count` silent frames into the stream checksum. The
/// checksum unit matches the playback sink: quantized little-endian
/// s16 stereo bytes, so silence contributes zero bytes and the running
/// FNV-1a state still advances deterministically.
pub fn capture_checksum_silence(checksum: u64, frame_count: usize) -> u64 {
    let mut hash = checksum;
    for _ in 0..frame_count.saturating_mul(4) {
        hash ^= 0u64;
        hash = hash.wrapping_mul(0x0010_0000_01b3);
    }
    hash
}

/// Evidence record for the boot selftest capture sweep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureSelftestResult {
    pub ok: bool,
    pub reads: usize,
    pub frames: usize,
    pub checksum: u64,
    pub first_frame_tick: u64,
    pub last_frame_tick: u64,
}

const SELFTEST_RATE_HZ: u32 = 48_000;
const SELFTEST_READ_FRAMES: usize = 512;
const SELFTEST_TICKS: u64 = 8;

/// Pace several reads against a synthetic clock exactly like the
/// service-side read loop does, proving timestamps advance with the
/// produced frames and the checksum folds over every returned frame.
pub fn run_capture_selftest() -> CaptureSelftestResult {
    let mut result = CaptureSelftestResult {
        ok: true,
        reads: 0,
        frames: 0,
        checksum: CHECKSUM_SEED,
        first_frame_tick: 0,
        last_frame_tick: 0,
    };
    let mut produced: u64 = 0;
    let mut previous_tick: u64 = 0;
    for tick in 1..=SELFTEST_TICKS {
        let due = capture_frames_due(produced, 0, tick, SELFTEST_RATE_HZ);
        let take = (due as usize).min(SELFTEST_READ_FRAMES);
        if take == 0 {
            continue;
        }
        let first_tick = capture_frame_tick(0, produced, SELFTEST_RATE_HZ);
        if result.reads == 0 {
            result.first_frame_tick = first_tick;
        } else if first_tick < previous_tick {
            result.ok = false;
        }
        previous_tick = capture_frame_tick(0, produced + take as u64 - 1, SELFTEST_RATE_HZ);
        result.last_frame_tick = previous_tick;
        result.checksum = capture_checksum_silence(result.checksum, take);
        produced += take as u64;
        result.reads += 1;
        result.frames += take;
    }
    result.ok = result.ok
        && result.frames > 0
        && result.checksum != CHECKSUM_SEED
        && result.last_frame_tick > result.first_frame_tick;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_is_zero_before_start_and_tracks_elapsed_time() {
        assert_eq!(capture_frames_due(0, 10, 10, 48_000), 0);
        assert_eq!(capture_frames_due(0, 10, 9, 48_000), 0);
        // One tick at 48 kHz owes 480 frames.
        assert_eq!(capture_frames_due(0, 10, 11, 48_000), 480);
        assert_eq!(capture_frames_due(0, 0, 3, 48_000), 1440);
        // Produced frames are credited against the time budget.
        assert_eq!(capture_frames_due(1000, 0, 3, 48_000), 440);
        assert_eq!(capture_frames_due(2000, 0, 3, 48_000), 0);
        assert_eq!(capture_frames_due(0, 0, 5, 0), 0);
    }

    #[test]
    fn due_never_regresses_for_monotonic_clock() {
        let mut produced = 0u64;
        let mut previous = 0u64;
        for tick in 0..50u64 {
            let due = capture_frames_due(produced, 0, tick, 16_000);
            assert!(due >= previous);
            let take = due.min(160);
            produced += take;
            previous = capture_frames_due(produced, 0, tick, 16_000);
        }
        // 160 frames per tick at 16 kHz keeps the source fully drained.
        assert_eq!(produced, 49 * 160);
    }

    #[test]
    fn frame_ticks_are_floor_paced() {
        assert_eq!(capture_frame_tick(7, 0, 48_000), 7);
        // Frame 479 at 48 kHz is still inside tick 0; frame 480 lands on 1.
        assert_eq!(capture_frame_tick(0, 479, 48_000), 0);
        assert_eq!(capture_frame_tick(0, 480, 48_000), 1);
        // At 100 Hz every frame owns one tick.
        assert_eq!(capture_frame_tick(0, 42, 100), 42);
        assert_eq!(capture_frame_tick(5, 10, 0), 5);
    }

    #[test]
    fn silence_packing_matches_format_widths() {
        // S16 stereo: two samples per frame, four samples per word.
        let mut words = [0u64; 16];
        assert_eq!(
            capture_pack_silence(AudioSampleFormat::S16Le, 2, 26, &mut words),
            13
        );
        assert!(words[..13].iter().all(|word| *word == 0));
        // U8 silence is 0x80 per lane (biased encoding), not zero bytes.
        words = [0u64; 16];
        assert_eq!(
            capture_pack_silence(AudioSampleFormat::U8, 1, 104, &mut words),
            13
        );
        assert_eq!(words[0], 0x8080_8080_8080_8080);
        // F32 zero-amplitude packs to zero bits; stereo needs one word
        // per frame so a 26-frame read fills the whole 16-word buffer.
        words = [0u64; 16];
        assert_eq!(
            capture_pack_silence(AudioSampleFormat::F32Le, 2, 26, &mut words),
            16
        );
        assert!(words.iter().all(|word| *word == 0));
        // Clamps to the destination capacity without panicking.
        let mut short = [0u64; 2];
        assert_eq!(
            capture_pack_silence(AudioSampleFormat::S16Le, 2, 1000, &mut short),
            2
        );
    }

    #[test]
    fn silence_checksum_matches_manual_fnv_over_zero_bytes() {
        let mut expected = CHECKSUM_SEED;
        for _ in 0..12 {
            expected ^= 0;
            expected = expected.wrapping_mul(0x0100_0000_1b3);
        }
        assert_eq!(capture_checksum_silence(CHECKSUM_SEED, 3), expected);
        assert_eq!(capture_checksum_silence(CHECKSUM_SEED, 0), CHECKSUM_SEED);
        assert_ne!(capture_checksum_silence(CHECKSUM_SEED, 1), CHECKSUM_SEED);
    }

    #[test]
    fn selftest_sweep_is_deterministic_and_paced() {
        let first = run_capture_selftest();
        let second = run_capture_selftest();
        assert_eq!(first, second);
        assert!(first.ok);
        assert!(first.reads > 1);
        assert!(first.frames >= first.reads);
        assert_ne!(first.checksum, CHECKSUM_SEED);
        assert!(first.last_frame_tick > first.first_frame_tick);
        // Real-time pacing: a read can never pull more than the elapsed
        // ticks justify, so every tick drains exactly its 480-frame
        // budget regardless of the 512-frame read cap.
        assert_eq!(first.reads, SELFTEST_TICKS as usize);
        assert_eq!(first.frames, SELFTEST_TICKS as usize * 480);
    }
}
