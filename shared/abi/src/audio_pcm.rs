//! Mixed-PCM output pipeline: sample-format codec, rate negotiation,
//! nearest-neighbor resampling, per-stream frame queues, volume/mute
//! curves, and a null sink that sums streams with clipping protection.
//!
//! Everything here is pure computation so it can be exercised by host
//! unit tests; the audio service wires it to IPC endpoints.

use crate::audio::AudioSampleFormat;

pub const SINK_RATE_HZ: u32 = 48_000;
pub const SUPPORTED_RATES: [u32; 7] = [8000, 11025, 16000, 22050, 32000, 44100, 48000];
pub const MIX_BATCH_FRAMES: usize = 256;
pub const PCM_RING_FRAMES: usize = 512;
pub const CHECKSUM_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// Number of interleaved samples packed into one 64-bit IPC word.
pub fn pcm_samples_per_word(format: AudioSampleFormat) -> usize {
    match format {
        AudioSampleFormat::U8 => 8,
        AudioSampleFormat::S16Le => 4,
        AudioSampleFormat::S32Le | AudioSampleFormat::F32Le => 2,
    }
}

pub fn pcm_decode_sample(format: AudioSampleFormat, raw: u32) -> f32 {
    match format {
        AudioSampleFormat::U8 => (raw as f32 - 128.0) / 128.0,
        AudioSampleFormat::S16Le => (raw as u16 as i16) as f32 / 32768.0,
        AudioSampleFormat::S32Le => (raw as i32) as f32 / 2147483648.0,
        AudioSampleFormat::F32Le => f32::from_bits(raw),
    }
}

/// Quantize to s16 with clamping to full scale (no wraparound).
pub fn pcm_encode_sample_s16(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * 32767.0) as i16
}

fn pcm_sample_bits(format: AudioSampleFormat, word: u64, index: usize) -> u32 {
    match format {
        AudioSampleFormat::U8 => ((word >> (8 * index)) & 0xff) as u32,
        AudioSampleFormat::S16Le => ((word >> (16 * index)) & 0xffff) as u32,
        AudioSampleFormat::S32Le | AudioSampleFormat::F32Le => match index {
            0 => word as u32,
            _ => (word >> 32) as u32,
        },
    }
}

/// Decode `sample_count` interleaved samples from packed IPC words.
pub fn pcm_decode_words(
    format: AudioSampleFormat,
    words: &[u64],
    sample_count: usize,
    out: &mut [f32],
) -> usize {
    let per_word = pcm_samples_per_word(format);
    let mut decoded = 0usize;
    for &word in words.iter() {
        for lane in 0..per_word {
            if decoded >= sample_count || decoded >= out.len() {
                return decoded;
            }
            out[decoded] = pcm_decode_sample(format, pcm_sample_bits(format, word, lane));
            decoded += 1;
        }
    }
    decoded
}

/// Pack interleaved samples into IPC words in the given format.
pub fn pcm_pack_words(format: AudioSampleFormat, samples: &[f32], words: &mut [u64]) -> usize {
    let per_word = pcm_samples_per_word(format);
    let mut word_count = 0usize;
    let mut cursor = 0usize;
    while cursor < samples.len() && word_count < words.len() {
        let take = (samples.len() - cursor).min(per_word);
        words[word_count] = pcm_pack_word(format, &samples[cursor..cursor + take]);
        word_count += 1;
        cursor += take;
    }
    word_count
}

fn pcm_pack_word(format: AudioSampleFormat, lane_samples: &[f32]) -> u64 {
    let mut word = 0u64;
    for (lane, value) in lane_samples.iter().enumerate() {
        match format {
            AudioSampleFormat::U8 => {
                let scaled = (*value).clamp(-1.0, 1.0) * 127.0 + 128.0;
                let raw = (scaled as i64).max(0).min(255) as u64;
                word |= raw << (8 * lane);
            }
            AudioSampleFormat::S16Le => {
                word |= (pcm_encode_sample_s16(*value) as u16 as u64) << (16 * lane);
            }
            AudioSampleFormat::S32Le => {
                let raw = ((*value).clamp(-1.0, 1.0) * 2147483647.0) as i32 as u32 as u64;
                word |= raw << (32 * lane);
            }
            AudioSampleFormat::F32Le => {
                word |= (value.to_bits() as u64) << (32 * lane);
            }
        }
    }
    word
}

/// Convert interleaved samples with `channels` layout into stereo frames.
/// Mono is duplicated; stereo passes through.
pub fn pcm_interleaved_to_stereo(samples: &[f32], channels: u32, out: &mut [[f32; 2]]) -> usize {
    if channels != 1 && channels != 2 {
        return 0;
    }
    let frame_count = samples.len() / channels as usize;
    let mut written = 0usize;
    for frame_index in 0..frame_count {
        if written >= out.len() {
            break;
        }
        let base = frame_index * channels as usize;
        let (left, right) = if channels == 1 {
            (samples[base], samples[base])
        } else {
            (samples[base], samples[base + 1])
        };
        out[written] = [left, right];
        written += 1;
    }
    written
}

/// Pick the closest supported rate for a requested stream rate.
pub fn pcm_nearest_supported_rate(requested: u32) -> u32 {
    let mut best = SUPPORTED_RATES[0];
    let mut best_delta = requested.abs_diff(best);
    for &rate in SUPPORTED_RATES.iter() {
        let delta = requested.abs_diff(rate);
        if delta < best_delta {
            best = rate;
            best_delta = delta;
        }
    }
    best
}

/// Output frame count when converting `input_frames` between rates.
pub fn pcm_resampled_len(input_frames: usize, src_rate: u32, dst_rate: u32) -> usize {
    if src_rate == 0 || dst_rate == 0 {
        return 0;
    }
    ((input_frames as u64 * dst_rate as u64) / src_rate as u64) as usize
}

/// Nearest-neighbor stereo frame resampler. Returns frames written.
pub fn pcm_resample_stereo(
    src: &[[f32; 2]],
    src_rate: u32,
    dst_rate: u32,
    dst: &mut [[f32; 2]],
) -> usize {
    if src.is_empty() || src_rate == 0 || dst_rate == 0 {
        return 0;
    }
    let out_frames = pcm_resampled_len(src.len(), src_rate, dst_rate).min(dst.len());
    for (out_index, slot) in dst[..out_frames].iter_mut().enumerate() {
        let src_index = (out_index as u64 * src_rate as u64 / dst_rate as u64) as usize;
        let src_index = src_index.min(src.len() - 1);
        *slot = src[src_index];
    }
    out_frames
}

/// Fixed-capacity FIFO of stereo f32 frames.
pub struct PcmFrameRing<const CAP: usize> {
    buf: [[f32; 2]; CAP],
    head: usize,
    len: usize,
}

impl<const CAP: usize> PcmFrameRing<CAP> {
    pub const fn new() -> Self {
        Self {
            buf: [[0.0; 2]; CAP],
            head: 0,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn free(&self) -> usize {
        CAP - self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    pub fn push_frame(&mut self, left: f32, right: f32) -> bool {
        if self.len >= CAP {
            return false;
        }
        let tail = (self.head + self.len) % CAP;
        self.buf[tail] = [left, right];
        self.len += 1;
        true
    }

    pub fn pop_frame(&mut self) -> Option<[f32; 2]> {
        if self.len == 0 {
            return None;
        }
        let frame = self.buf[self.head];
        self.head = (self.head + 1) % CAP;
        self.len -= 1;
        Some(frame)
    }
}

/// Per-stream PCM state: negotiated format, volume/mute, queue ring,
/// and counters proving how much audio flowed through the stream.
pub struct PcmStreamState {
    pub active: bool,
    pub configured: bool,
    pub format: AudioSampleFormat,
    pub rate_hz: u32,
    pub channels: u32,
    pub volume: u8,
    pub muted: bool,
    pub frames_written: u64,
    pub checksum: u64,
    pub ring: PcmFrameRing<PCM_RING_FRAMES>,
}

impl PcmStreamState {
    pub const fn new() -> Self {
        Self {
            active: false,
            configured: false,
            format: AudioSampleFormat::S16Le,
            rate_hz: 0,
            channels: 0,
            volume: 100,
            muted: false,
            frames_written: 0,
            checksum: CHECKSUM_SEED,
            ring: PcmFrameRing::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Validate a configuration request; returns the accepted
    /// (format, rate, channels) triple or None when unsupported.
    pub fn negotiate(
        format: AudioSampleFormat,
        requested_rate: u32,
        channels: u32,
    ) -> Option<(AudioSampleFormat, u32, u32)> {
        if requested_rate == 0 || (channels != 1 && channels != 2) {
            return None;
        }
        Some((format, pcm_nearest_supported_rate(requested_rate), channels))
    }

    pub fn apply_config(&mut self, format: AudioSampleFormat, rate_hz: u32, channels: u32) {
        self.format = format;
        self.rate_hz = rate_hz;
        self.channels = channels;
        self.configured = true;
        self.ring.clear();
        self.frames_written = 0;
        self.checksum = CHECKSUM_SEED;
    }

    /// Decode one packed IPC chunk and queue it as sink-rate stereo frames.
    /// Returns queued frame count, or None when the ring lacks space.
    pub fn ingest_chunk(&mut self, words: &[u64], sample_count: usize) -> Option<usize> {
        if !self.configured || sample_count == 0 {
            return Some(0);
        }
        let chunk_frames = sample_count / self.channels as usize;
        let needed = pcm_resampled_len(chunk_frames, self.rate_hz, SINK_RATE_HZ);
        if self.ring.free() < needed {
            return None;
        }
        let mut samples = [0.0f32; 128];
        let decoded = pcm_decode_words(self.format, words, sample_count, &mut samples);
        let mut stereo = [[0.0f32; 2]; 128];
        let frames = pcm_interleaved_to_stereo(&samples[..decoded], self.channels, &mut stereo);
        let mut resampled = [[0.0f32; 2]; 128];
        let out_frames = pcm_resample_stereo(
            &stereo[..frames],
            self.rate_hz,
            SINK_RATE_HZ,
            &mut resampled,
        );
        for frame in resampled[..out_frames].iter() {
            if !self.ring.push_frame(frame[0], frame[1]) {
                break;
            }
            self.frames_written += 1;
        }
        Some(out_frames)
    }
}

/// Perceptual volume curve: 0..100 percent mapped to quadratic gain,
/// with mute forcing silence.
pub fn pcm_volume_gain(volume: u8, muted: bool) -> f32 {
    if muted {
        return 0.0;
    }
    let v = volume.min(100) as f32 / 100.0;
    v * v
}

fn pcm_fnv1a(checksum: u64, bytes: &[u8]) -> u64 {
    let mut hash = checksum;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_1b3);
    }
    hash
}

/// Quantize a stereo pair to little-endian s16 bytes (checksum unit).
pub fn pcm_quantize_stereo_bytes(left: f32, right: f32) -> [u8; 4] {
    let l = pcm_encode_sample_s16(left).to_le_bytes();
    let r = pcm_encode_sample_s16(right).to_le_bytes();
    [l[0], l[1], r[0], r[1]]
}

/// Null PCM sink: sums every configured stream, applies master volume,
/// clamps to full scale, and folds the result into counters plus an
/// FNV-1a checksum that proves end-to-end flow without audible hardware.
pub struct PcmNullSink {
    pub master_volume: u8,
    pub master_muted: bool,
    pub frames_mixed: u64,
    pub clipped_frames: u64,
    pub checksum: u64,
}

impl PcmNullSink {
    pub const fn new() -> Self {
        Self {
            master_volume: 100,
            master_muted: false,
            frames_mixed: 0,
            clipped_frames: 0,
            checksum: CHECKSUM_SEED,
        }
    }

    /// Mix up to `max_frames` output frames from all active streams.
    pub fn mix_batch(&mut self, streams: &mut [PcmStreamState], max_frames: usize) -> usize {
        // Delegate through mix_batch_into so counters/checksums can never
        // drift between the capture-free and byte-capturing paths.
        let mut no_bytes = [0u8; 0];
        self.mix_batch_into(streams, max_frames, &mut no_bytes)
    }

    /// Mix like [`PcmNullSink::mix_batch`] and additionally copy each
    /// mixed frame's quantized s16le stereo bytes into `out_bytes`
    /// (4 bytes per frame) while space remains. Counters and checksums
    /// are identical to the plain path regardless of `out_bytes` size.
    pub fn mix_batch_into(
        &mut self,
        streams: &mut [PcmStreamState],
        max_frames: usize,
        out_bytes: &mut [u8],
    ) -> usize {
        let mut mixed = 0usize;
        for _ in 0..max_frames {
            let mut acc = [0.0f32; 2];
            let mut any = false;
            for stream in streams.iter_mut() {
                if !stream.active || !stream.configured {
                    continue;
                }
                if let Some(frame) = stream.ring.pop_frame() {
                    any = true;
                    let gain = pcm_volume_gain(stream.volume, stream.muted);
                    let left = frame[0] * gain;
                    let right = frame[1] * gain;
                    let contribution = pcm_quantize_stereo_bytes(left, right);
                    stream.checksum = pcm_fnv1a(stream.checksum, &contribution);
                    acc[0] += left;
                    acc[1] += right;
                }
            }
            if !any {
                break;
            }
            let master_gain = pcm_volume_gain(self.master_volume, self.master_muted);
            let left = (acc[0] * master_gain).clamp(-1.0, 1.0);
            let right = (acc[1] * master_gain).clamp(-1.0, 1.0);
            if left >= 1.0 || right >= 1.0 || left <= -1.0 || right <= -1.0 {
                self.clipped_frames += 1;
            }
            let mixed_bytes = pcm_quantize_stereo_bytes(left, right);
            self.checksum = pcm_fnv1a(self.checksum, &mixed_bytes);
            let slot = mixed * 4;
            if slot + 4 <= out_bytes.len() {
                out_bytes[slot..slot + 4].copy_from_slice(&mixed_bytes);
            }
            self.frames_mixed += 1;
            mixed += 1;
        }
        mixed
    }

    /// Drain every queued frame from all streams; returns frames mixed.
    pub fn mix_until_empty(&mut self, streams: &mut [PcmStreamState]) -> usize {
        let mut total = 0usize;
        loop {
            let mixed = self.mix_batch(streams, MIX_BATCH_FRAMES);
            total += mixed;
            if mixed < MIX_BATCH_FRAMES {
                return total;
            }
        }
    }
}

/// Boot selftest result summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PcmMixSelftestResult {
    pub ok: bool,
    pub frames_mixed: usize,
    pub clipped_frames: u64,
    pub checksum_a: u64,
    pub checksum_b: u64,
    pub checksum_mixed: u64,
}

fn failed_result() -> PcmMixSelftestResult {
    PcmMixSelftestResult {
        ok: false,
        frames_mixed: 0,
        clipped_frames: 0,
        checksum_a: 0,
        checksum_b: 0,
        checksum_mixed: 0,
    }
}

// One message carries at most 16 words, so chunks stay within 14 payload
// words (samples start at word 2).
const SELFTEST_FRAMES_A: usize = 48; // mono s16 @ sink rate -> one message.
const SELFTEST_B_CHUNKS: usize = 4;
const SELFTEST_B_FRAMES_PER_CHUNK: usize = 12; // stereo f32 @ half rate.

fn selftest_ingest_streams(streams: &mut [PcmStreamState; 2]) -> bool {
    *streams = [
        PcmStreamState::new(),
        PcmStreamState::new(),
    ];
    streams[0].active = true;
    streams[0].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
    streams[1].active = true;
    streams[1].apply_config(AudioSampleFormat::F32Le, 24_000, 2);
    streams[1].volume = 50;

    let mut packed = [0.0f32; 128];
    let mut words = [0u64; 14];

    for frame in 0..SELFTEST_FRAMES_A {
        packed[frame] = 0.6 * (((frame % 16) as f32 / 8.0) - 1.0);
    }
    let word_count = pcm_pack_words(
        AudioSampleFormat::S16Le,
        &packed[..SELFTEST_FRAMES_A],
        &mut words,
    );
    if streams[0].ingest_chunk(&words[..word_count], SELFTEST_FRAMES_A)
        != Some(SELFTEST_FRAMES_A)
    {
        return false;
    }

    for chunk in 0..SELFTEST_B_CHUNKS {
        for local in 0..SELFTEST_B_FRAMES_PER_CHUNK {
            let frame = chunk * SELFTEST_B_FRAMES_PER_CHUNK + local;
            let value = if frame % 2 == 0 { -0.5 } else { 0.5 };
            packed[local * 2] = value;
            packed[local * 2 + 1] = -value;
        }
        let word_count = pcm_pack_words(
            AudioSampleFormat::F32Le,
            &packed[..SELFTEST_B_FRAMES_PER_CHUNK * 2],
            &mut words,
        );
        if streams[1].ingest_chunk(
            &words[..word_count],
            SELFTEST_B_FRAMES_PER_CHUNK * 2,
        ) != Some(SELFTEST_B_FRAMES_PER_CHUNK * 2)
        {
            return false;
        }
    }
    true
}

fn selftest_expected_frames() -> usize {
    let expected_b = selftest_expected_stream_b_frames();
    expected_b.max(SELFTEST_FRAMES_A)
}

fn selftest_result(
    mixed: usize,
    sink_checksum: u64,
    clipped_frames: u64,
    streams: &[PcmStreamState; 2],
) -> PcmMixSelftestResult {
    let expected_mixed = selftest_expected_frames();
    let ok = mixed == expected_mixed
        && clipped_frames == 0
        && streams[0].checksum != CHECKSUM_SEED
        && streams[1].checksum != CHECKSUM_SEED
        && sink_checksum != CHECKSUM_SEED
        && mixed > 0;
    PcmMixSelftestResult {
        ok,
        frames_mixed: mixed,
        clipped_frames,
        checksum_a: streams[0].checksum,
        checksum_b: streams[1].checksum,
        checksum_mixed: sink_checksum,
    }
}

/// Boot selftest: push two concurrent synthetic streams (different rates,
/// channel counts, and volumes) through the real decode/resample/queue/mix
/// pipeline and verify the null sink saw both contributions without
/// clipping. Pure computation; never touches hardware endpoints.
pub fn run_pcm_mix_selftest() -> PcmMixSelftestResult {
    let mut streams = [PcmStreamState::new(), PcmStreamState::new()];
    if !selftest_ingest_streams(&mut streams) {
        return failed_result();
    }

    let mut sink = PcmNullSink::new();
    let mixed = sink.mix_until_empty(&mut streams);

    let result = selftest_result(mixed, sink.checksum, sink.clipped_frames, &streams);
    if sink.frames_mixed as usize != selftest_expected_frames()
        || streams[0].frames_written as usize != SELFTEST_FRAMES_A
        || streams[1].frames_written as usize != selftest_expected_stream_b_frames()
    {
        return failed_result();
    }
    result
}

fn selftest_expected_stream_b_frames() -> usize {
    let total_b_frames = SELFTEST_B_CHUNKS * SELFTEST_B_FRAMES_PER_CHUNK;
    total_b_frames * 2 // B arrives at half the sink rate.
}

/// Hardware-path variant of [`run_pcm_mix_selftest`]: replays the same two
/// synthetic streams but hands every mixed batch's quantized s16le stereo
/// bytes to `emit`, exactly as a real PCM sink would consume them.
/// `PcmMixSelftestResult::frames_mixed` doubles as the emitted-frame count.
pub fn run_pcm_mix_selftest_emit(emit: &mut dyn FnMut(&[u8])) -> PcmMixSelftestResult {
    let mut streams = [PcmStreamState::new(), PcmStreamState::new()];
    if !selftest_ingest_streams(&mut streams) {
        return failed_result();
    }

    let mut sink = PcmNullSink::new();
    let mut batch = [0u8; MIX_BATCH_FRAMES * 4];
    let mut emitted_frames = 0usize;
    loop {
        let mixed = sink.mix_batch_into(&mut streams, MIX_BATCH_FRAMES, &mut batch);
        if mixed == 0 {
            break;
        }
        emit(&batch[..mixed * 4]);
        emitted_frames += mixed;
        if mixed < MIX_BATCH_FRAMES {
            break;
        }
    }

    let result = selftest_result(emitted_frames, sink.checksum, sink.clipped_frames, &streams);
    if sink.frames_mixed as usize != emitted_frames
        || streams[1].frames_written as usize != selftest_expected_stream_b_frames()
    {
        return failed_result();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_curve_endpoints_and_mute() {
        assert_eq!(pcm_volume_gain(0, false), 0.0);
        assert_eq!(pcm_volume_gain(100, false), 1.0);
        assert_eq!(pcm_volume_gain(50, false), 0.25);
        assert_eq!(pcm_volume_gain(100, true), 0.0);
        assert_eq!(pcm_volume_gain(37, true), 0.0);
        let mut previous = 0.0f32;
        for percent in 0..=100u8 {
            let gain = pcm_volume_gain(percent, false);
            assert!(gain >= previous);
            assert!((0.0..=1.0).contains(&gain));
            previous = gain;
        }
    }

    #[test]
    fn samples_per_word_matches_format_width() {
        assert_eq!(pcm_samples_per_word(AudioSampleFormat::U8), 8);
        assert_eq!(pcm_samples_per_word(AudioSampleFormat::S16Le), 4);
        assert_eq!(pcm_samples_per_word(AudioSampleFormat::S32Le), 2);
        assert_eq!(pcm_samples_per_word(AudioSampleFormat::F32Le), 2);
    }

    #[test]
    fn s16_roundtrip_preserves_values() {
        let samples: [f32; 8] = [0.0, 0.5, -0.5, 1.0, -1.0, 0.25, -0.75, 0.125];
        let mut words = [0u64; 4];
        let word_count = pcm_pack_words(AudioSampleFormat::S16Le, &samples, &mut words);
        assert_eq!(word_count, 2);
        let mut decoded = [0f32; 8];
        let count = pcm_decode_words(AudioSampleFormat::S16Le, &words, 8, &mut decoded);
        assert_eq!(count, 8);
        for (original, back) in samples.iter().zip(decoded.iter()) {
            let expected = pcm_encode_sample_s16(*original) as i32 as f32 / 32768.0;
            assert_eq!(expected, *back);
        }
        // Clamped quantization never wraps past full scale.
        assert_eq!(pcm_encode_sample_s16(2.0), 32767);
        assert_eq!(pcm_encode_sample_s16(-2.0), -32767);
    }

    #[test]
    fn u8_decode_uses_offset_binary() {
        let words = [128u64 | (255u64 << 8) | (0u64 << 16)];
        let mut decoded = [0f32; 3];
        let count = pcm_decode_words(AudioSampleFormat::U8, &words, 3, &mut decoded);
        assert_eq!(count, 3);
        assert!(decoded[0].abs() < 1e-6);
        assert!((decoded[1] - 127.0 / 128.0).abs() < 1e-6);
        assert!((decoded[2] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn f32_decode_is_bit_exact() {
        let values = [0.25f32, -0.5f32];
        let mut words = [0u64; 1];
        let word_count = pcm_pack_words(AudioSampleFormat::F32Le, &values, &mut words);
        assert_eq!(word_count, 1);
        let mut decoded = [0f32; 2];
        pcm_decode_words(AudioSampleFormat::F32Le, &words, 2, &mut decoded);
        assert_eq!(decoded[0], 0.25);
        assert_eq!(decoded[1], -0.5);
    }

    #[test]
    fn channel_layout_folding() {
        let mut out = [[0f32; 2]; 4];
        let frames = pcm_interleaved_to_stereo(&[0.1, 0.2, 0.3], 1, &mut out);
        assert_eq!(frames, 3);
        assert_eq!(out[0], [0.1, 0.1]);
        assert_eq!(out[2], [0.3, 0.3]);
        let frames = pcm_interleaved_to_stereo(&[0.1, 0.9], 2, &mut out);
        assert_eq!(frames, 1);
        assert_eq!(out[0], [0.1, 0.9]);
        assert_eq!(pcm_interleaved_to_stereo(&[0.5; 6], 3, &mut out), 0);
    }

    #[test]
    fn nearest_rate_negotiation_picks_closest_supported() {
        assert_eq!(pcm_nearest_supported_rate(48_000), 48_000);
        assert_eq!(pcm_nearest_supported_rate(44_100), 44_100);
        assert_eq!(pcm_nearest_supported_rate(23_000), 22_050);
        assert_eq!(pcm_nearest_supported_rate(1), 8000);
        assert_eq!(pcm_nearest_supported_rate(96_000), 48_000);
    }

    #[test]
    fn ring_is_fifo_with_wraparound() {
        let mut ring: PcmFrameRing<4> = PcmFrameRing::new();
        assert!(ring.is_empty());
        assert_eq!(ring.free(), 4);
        for index in 0..4u32 {
            assert!(ring.push_frame(index as f32, -(index as f32)));
        }
        assert!(!ring.push_frame(99.0, 99.0));
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.pop_frame(), Some([0.0, 0.0]));
        assert!(ring.push_frame(4.0, -4.0));
        assert_eq!(ring.free(), 0);
        let mut drained = [[0f32; 2]; 8];
        let drained_len = {
            let mut count = 0usize;
            while let Some(frame) = ring.pop_frame() {
                drained[count] = frame;
                count += 1;
            }
            count
        };
        assert_eq!(
            &drained[..drained_len],
            &[[1.0, -1.0], [2.0, -2.0], [3.0, -3.0], [4.0, -4.0]]
        );
        ring.clear();
        assert!(ring.is_empty());
    }

    #[test]
    fn resampler_doubles_half_rate_input() {
        let src = [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]];
        assert_eq!(pcm_resampled_len(3, 24_000, 48_000), 6);
        let mut dst = [[0f32; 2]; 8];
        let written = pcm_resample_stereo(&src, 24_000, 48_000, &mut dst);
        assert_eq!(written, 6);
        // Nearest-neighbor maps each output index onto one input frame.
        assert_eq!(dst[0], [0.0, 0.0]);
        assert_eq!(dst[1], [0.0, 0.0]);
        assert_eq!(dst[2], [1.0, 1.0]);
        assert_eq!(dst[3], [1.0, 1.0]);
        assert_eq!(dst[4], [2.0, 2.0]);
        assert_eq!(dst[5], [2.0, 2.0]);
        let mut same = [[0f32; 2]; 8];
        let written = pcm_resample_stereo(&src, SINK_RATE_HZ, SINK_RATE_HZ, &mut same);
        assert_eq!(written, 3);
        assert_eq!(same[1], [1.0, 1.0]);
    }

    #[test]
    fn two_stream_mix_matches_reference_sum() {
        const FRAMES: usize = 28; // stereo s16 fits one message (14 words).
        let mut streams = [PcmStreamState::new(), PcmStreamState::new()];
        streams[0].active = true;
        streams[0].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        streams[0].volume = 100;
        streams[1].active = true;
        streams[1].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 2);
        streams[1].volume = 50;

        let a: [f32; FRAMES] = core::array::from_fn(|index| ((index % 8) as f32) * 0.05 - 0.15);
        let mut b = [0f32; FRAMES * 2];
        for index in 0..FRAMES {
            b[index * 2] = -0.4;
            b[index * 2 + 1] = 0.4 * ((index % 4) as f32);
        }

        let mut words = [0u64; 14];
        let count = pcm_pack_words(AudioSampleFormat::S16Le, &a, &mut words);
        assert_eq!(
            streams[0].ingest_chunk(&words[..count], FRAMES),
            Some(FRAMES)
        );
        let count = pcm_pack_words(AudioSampleFormat::S16Le, &b, &mut words);
        assert_eq!(
            streams[1].ingest_chunk(&words[..count], FRAMES * 2),
            Some(FRAMES)
        );

        let mut sink = PcmNullSink::new();
        let mixed = sink.mix_until_empty(&mut streams);
        assert_eq!(mixed, FRAMES);

        // Independent reference: per-frame post-gain sum with master gain,
        // clamped to full scale before quantization.
        let gain_a = pcm_volume_gain(100, false);
        let gain_b = pcm_volume_gain(50, false);
        for index in 0..FRAMES {
            let left = a[index] * gain_a + b[index * 2] * gain_b;
            let right = a[index] * gain_a + b[index * 2 + 1] * gain_b;
            let expected = pcm_quantize_stereo_bytes(left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
            assert_ne!(expected, [0u8; 4]);
        }
        assert_ne!(sink.checksum, CHECKSUM_SEED);
        assert_eq!(sink.frames_mixed as usize, FRAMES);
        assert_eq!(sink.clipped_frames, 0);
        assert_eq!(streams[0].frames_written as usize, FRAMES);
        assert_eq!(streams[1].frames_written as usize, FRAMES);
        assert_ne!(streams[0].checksum, CHECKSUM_SEED);
        assert_ne!(streams[1].checksum, CHECKSUM_SEED);
        assert_ne!(streams[0].checksum, streams[1].checksum);
    }

    #[test]
    fn loud_streams_clip_instead_of_wrapping() {
        const FRAMES: usize = 8;
        let mut streams = [
            PcmStreamState::new(),
            PcmStreamState::new(),
            PcmStreamState::new(),
            PcmStreamState::new(),
        ];
        for stream in streams.iter_mut() {
            stream.active = true;
            stream.apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
            stream.volume = 100;
        }
        let mut words = [0u64; 14];
        let samples = [1.0f32; FRAMES];
        let count = pcm_pack_words(AudioSampleFormat::S16Le, &samples, &mut words);
        for stream in streams.iter_mut() {
            assert_eq!(stream.ingest_chunk(&words[..count], FRAMES), Some(FRAMES));
        }
        let mut sink = PcmNullSink::new();
        let mixed = sink.mix_until_empty(&mut streams);
        assert_eq!(mixed, FRAMES);
        assert_eq!(sink.clipped_frames, FRAMES as u64);
        // Clamped output is exactly full-scale s16.
        assert_eq!(
            pcm_quantize_stereo_bytes(4.0, 4.0),
            [0xff, 0x7f, 0xff, 0x7f]
        );
        assert_eq!(
            pcm_quantize_stereo_bytes(-4.0, -4.0),
            [0x01, 0x80, 0x01, 0x80]
        );
    }

    #[test]
    fn muted_streams_contribute_silence_but_still_flow() {
        let mut streams = [PcmStreamState::new()];
        streams[0].active = true;
        streams[0].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        streams[0].muted = true;
        let mut words = [0u64; 14];
        let samples = [0.5f32; 8];
        let count = pcm_pack_words(AudioSampleFormat::S16Le, &samples, &mut words);
        assert_eq!(streams[0].ingest_chunk(&words[..count], 8), Some(8));
        let mut sink = PcmNullSink::new();
        let mixed = sink.mix_until_empty(&mut streams);
        assert_eq!(mixed, 8);
        assert_eq!(sink.frames_mixed, 8);
        // Master mute silences the sink while frames still flow through it.
        let mut master = PcmNullSink::new();
        master.master_muted = true;
        let mut loud = PcmStreamState::new();
        loud.active = true;
        loud.apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        let _ = loud.ingest_chunk(&words[..count], 8);
        let mixed = master.mix_until_empty(core::slice::from_mut(&mut loud));
        assert_eq!(mixed, 8);
    }

    #[test]
    fn full_ring_rejects_further_chunks() {
        let mut stream = PcmStreamState::new();
        stream.apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        stream.active = true;
        assert_eq!(pcm_resampled_len(10, SINK_RATE_HZ, SINK_RATE_HZ), 10);
        loop {
            let mut words = [0u64; 14];
            let samples = [0.1f32; 56];
            let count = pcm_pack_words(AudioSampleFormat::S16Le, &samples, &mut words);
            if stream.ingest_chunk(&words[..count], 56).is_none() {
                break;
            }
        }
        assert!(stream.ring.free() < 60);
        assert_eq!(stream.frames_written as usize >= PCM_RING_FRAMES - 60, true);
    }

    #[test]
    fn unconfigured_and_invalid_configs_are_rejected() {
        let mut stream = PcmStreamState::new();
        let words = [0u64; 14];
        assert_eq!(stream.ingest_chunk(&words, 4), Some(0));
        assert!(PcmStreamState::negotiate(AudioSampleFormat::S16Le, 0, 1).is_none());
        assert!(PcmStreamState::negotiate(AudioSampleFormat::S16Le, 48_000, 0).is_none());
        assert!(PcmStreamState::negotiate(AudioSampleFormat::S16Le, 48_000, 7).is_none());
        let accepted = PcmStreamState::negotiate(AudioSampleFormat::F32Le, 24_000, 2);
        assert_eq!(
            accepted,
            Some((
                AudioSampleFormat::F32Le,
                pcm_nearest_supported_rate(24_000),
                2
            ))
        );
    }

    #[test]
    fn mix_batch_respects_frame_budget() {
        let mut streams = [PcmStreamState::new()];
        streams[0].active = true;
        streams[0].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        let mut words = [0u64; 14];
        let samples = [0.25f32; 56]; // exactly 14 s16-packed words.
        let count = pcm_pack_words(AudioSampleFormat::S16Le, &samples, &mut words);
        assert_eq!(count, 14);
        assert_eq!(streams[0].ingest_chunk(&words[..count], 56), Some(56));
        let mut sink = PcmNullSink::new();
        let mixed = sink.mix_batch(&mut streams, 50);
        assert_eq!(mixed, 50);
        assert_eq!(streams[0].ring.len(), 6);
        let rest = sink.mix_until_empty(&mut streams);
        assert_eq!(rest, 6);
        assert_eq!(sink.frames_mixed, 56);
    }

    #[test]
    fn boot_selftest_two_streams_mix_cleanly() {
        let result = run_pcm_mix_selftest();
        assert!(result.ok, "selftest failed: {result:?}");
    }

    #[test]
    fn boot_selftest_emit_variant_flows_identical_frames() {
        const CAP: usize = 512;
        let mut bytes = [0u8; CAP];
        let mut written = 0usize;
        let result = run_pcm_mix_selftest_emit(&mut |batch| {
            assert_eq!(batch.len() % 4, 0);
            assert!(written + batch.len() <= CAP, "emit overflow");
            bytes[written..written + batch.len()].copy_from_slice(batch);
            written += batch.len();
        });
        assert!(result.ok, "emit selftest failed: {result:?}");
        // Every mixed frame is quantized to 4 s16le stereo bytes.
        assert_eq!(written, result.frames_mixed * 4);

        let reference = run_pcm_mix_selftest();
        assert_eq!(
            (reference.frames_mixed, reference.checksum_mixed),
            (result.frames_mixed, result.checksum_mixed)
        );
    }

    #[test]
    fn mix_batch_into_matches_plain_sink_counters() {
        const FRAMES: usize = 24; // both chunks must fit one 14-word message.
        let feed = |streams: &mut [PcmStreamState; 2]| {
            let a: [f32; FRAMES] =
                core::array::from_fn(|index| ((index % 10) as f32) * 0.04 - 0.2);
            let mut b = [0f32; FRAMES * 2];
            for index in 0..FRAMES {
                b[index * 2] = 0.3;
                b[index * 2 + 1] = -0.3;
            }
            let mut words = [0u64; 14];
            let count = pcm_pack_words(AudioSampleFormat::S16Le, &a, &mut words);
            assert_eq!(streams[0].ingest_chunk(&words[..count], FRAMES), Some(FRAMES));
            let count = pcm_pack_words(AudioSampleFormat::S16Le, &b, &mut words);
            assert_eq!(
                streams[1].ingest_chunk(&words[..count], FRAMES * 2),
                Some(FRAMES)
            );
        };

        let mut captured = [PcmStreamState::new(), PcmStreamState::new()];
        captured[0].active = true;
        captured[0].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        captured[1].active = true;
        captured[1].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 2);
        captured[1].volume = 40;
        feed(&mut captured);

        let mut byte_sink = PcmNullSink::new();
        let mut out = [0xA5u8; MIX_BATCH_FRAMES * 4];
        let mixed = byte_sink.mix_batch_into(&mut captured, MIX_BATCH_FRAMES, &mut out);
        assert_eq!(mixed, FRAMES);
        assert_eq!(&out[mixed * 4..], &[0xA5; MIX_BATCH_FRAMES * 4 - FRAMES * 4][..]);

        // Identically fed streams through the plain path must produce the
        // same counters/checksums.
        let mut plain = [PcmStreamState::new(), PcmStreamState::new()];
        plain[0].active = true;
        plain[0].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 1);
        plain[1].active = true;
        plain[1].apply_config(AudioSampleFormat::S16Le, SINK_RATE_HZ, 2);
        plain[1].volume = 40;
        feed(&mut plain);
        let mut null_sink = PcmNullSink::new();
        let plain_mixed = null_sink.mix_until_empty(&mut plain);

        assert_eq!(plain_mixed, mixed);
        assert_eq!(null_sink.checksum, byte_sink.checksum);
        assert_eq!(null_sink.frames_mixed, byte_sink.frames_mixed);
        assert_eq!(null_sink.clipped_frames, byte_sink.clipped_frames);
        assert_eq!(captured[0].checksum, plain[0].checksum);
        assert_eq!(captured[1].checksum, plain[1].checksum);
        // Captured bytes decode back to the per-frame reference sum.
        // Both inputs are s16 codec round-tripped before mixing, so the
        // reference applies the same encode step.
        let gain_a = pcm_volume_gain(100, false);
        let gain_b = pcm_volume_gain(40, false);
        let b_roundtrip = pcm_encode_sample_s16(0.3) as f32 / 32768.0;
        for index in 0..FRAMES {
            let a_roundtrip = pcm_encode_sample_s16(a_value(index)) as f32 / 32768.0;
            let left = a_roundtrip * gain_a + b_roundtrip * gain_b;
            let right = a_roundtrip * gain_a - b_roundtrip * gain_b;
            let expected = pcm_quantize_stereo_bytes(
                left.clamp(-1.0, 1.0),
                right.clamp(-1.0, 1.0),
            );
            assert_eq!(&out[index * 4..index * 4 + 4], &expected);
        }
    }

    fn a_value(index: usize) -> f32 {
        ((index % 10) as f32) * 0.04 - 0.2
    }
}
