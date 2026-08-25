use serviceos_userspace_runtime::{AudioSampleFormat, IPC_MAX_WORDS, pcm_samples_per_word};

/// StreamWrite requests carry two header words before packed samples.
pub(crate) const WRITE_HEADER_WORDS: usize = 2;
/// The audio-service decodes at most 128 samples per ingest buffer.
pub(crate) const SERVICE_SAMPLE_BUFFER: usize = 128;

/// Largest packed-sample word count a single StreamWrite may carry.
pub(crate) fn max_chunk_words() -> usize {
    IPC_MAX_WORDS - WRITE_HEADER_WORDS
}

/// Frames per write chunk so one request never exceeds either the IPC word
/// budget or the service-side decode buffer. Always >= 1.
pub(crate) fn frames_per_chunk(channels: u32, format: AudioSampleFormat) -> usize {
    let channels = channels.max(1) as usize;
    let per_word = pcm_samples_per_word(format);
    let by_words = max_chunk_words().saturating_mul(per_word) / channels;
    let by_buffer = SERVICE_SAMPLE_BUFFER / channels;
    (by_words.min(by_buffer)).max(1)
}

/// Samples this chunk actually carries (clipped at the tail).
pub(crate) fn chunk_sample_count(
    remaining_frames: usize,
    frames_per_chunk: usize,
    channels: u32,
) -> usize {
    let frames = remaining_frames.min(frames_per_chunk);
    frames.saturating_mul(channels as usize)
}

/// Words needed to pack `sample_count` samples in `format`.
pub(crate) fn packed_word_count(sample_count: usize, format: AudioSampleFormat) -> usize {
    let per_word = pcm_samples_per_word(format);
    sample_count.div_ceil(per_word)
}

/// Number of writes needed for `total_frames` at the given chunk size.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn total_chunks(total_frames: usize, frames_per_chunk: usize) -> usize {
    if total_frames == 0 || frames_per_chunk == 0 {
        return 0;
    }
    total_frames.div_ceil(frames_per_chunk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_s16_chunks_fit_ipc_and_service_buffers() {
        let fpc = frames_per_chunk(2, AudioSampleFormat::S16Le);
        // 4 samples/word * 14 words = 56 samples = 28 stereo frames.
        assert_eq!(fpc, 28);
        let words = packed_word_count(chunk_sample_count(fpc, fpc, 2), AudioSampleFormat::S16Le);
        assert_eq!(words, max_chunk_words());
        // 28 stereo frames = 56 samples, under the service decode buffer.
        assert_eq!(chunk_sample_count(fpc, fpc, 2), 56);
        assert!(chunk_sample_count(fpc, fpc, 2) <= SERVICE_SAMPLE_BUFFER);
    }

    #[test]
    fn mono_u8_chunks_hit_the_decode_buffer_not_words() {
        let fpc = frames_per_chunk(1, AudioSampleFormat::U8);
        // Words would allow 8*14=112 samples; the service buffer caps at 128
        // but words win here.
        assert_eq!(fpc, 112);
        assert!(chunk_sample_count(fpc, fpc, 1) <= SERVICE_SAMPLE_BUFFER);
    }

    #[test]
    fn s32_stereo_is_word_limited_to_fourteen_frames() {
        let fpc = frames_per_chunk(2, AudioSampleFormat::S32Le);
        assert_eq!(fpc, 14);
        let fpc_mono = frames_per_chunk(1, AudioSampleFormat::F32Le);
        assert_eq!(fpc_mono, SERVICE_SAMPLE_BUFFER.min(max_chunk_words() * 2));
    }

    #[test]
    fn degenerate_inputs_stay_safe() {
        assert_eq!(frames_per_chunk(0, AudioSampleFormat::S16Le), 56);
        assert_eq!(chunk_sample_count(3, 28, 2), 6);
        assert_eq!(chunk_sample_count(0, 28, 2), 0);
        assert_eq!(total_chunks(0, 28), 0);
        assert_eq!(total_chunks(57, 28), 3);
        assert_eq!(total_chunks(56, 28), 2);
        // Tail chunks pack fewer words than the budget.
        assert_eq!(
            packed_word_count(chunk_sample_count(3, 28, 2), AudioSampleFormat::S16Le),
            2
        );
        assert_eq!(packed_word_count(5, AudioSampleFormat::U8), 1);
        assert_eq!(packed_word_count(9, AudioSampleFormat::U8), 2);
    }
}
