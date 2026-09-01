use serviceos_userspace_runtime::AudioSampleFormat;

/// Parsed WAV header. Only uncompressed PCM (format tag 1, bits 8/16/32),
/// IEEE float 32 (tag 3), and G.711 A-law/mu-law (tags 6/7, always 8-bit
/// code bytes) are supported; anything else is rejected honestly rather
/// than played as noise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WavInfo {
    pub(crate) channels: u32,
    pub(crate) sample_rate: u32,
    pub(crate) bits_per_sample: u32,
    pub(crate) is_float: bool,
    /// Raw fmt-chunk encoding tag: 1 PCM, 3 float, 0x11 IMA ADPCM.
    pub(crate) format_tag: u16,
    /// Declared block alignment from the fmt chunk.
    pub(crate) block_align: u32,
    /// wSamplesPerBlock for compressed formats; 0 when absent.
    pub(crate) samples_per_block: u32,
    pub(crate) data_offset: usize,
    pub(crate) data_len: usize,
}

impl WavInfo {
    /// Bytes per frame across all channels.
    pub(crate) fn frame_bytes(&self) -> usize {
        self.channels as usize * (self.bits_per_sample as usize / 8)
    }

    pub(crate) fn frame_count(&self) -> usize {
        let frame = self.frame_bytes();
        if frame == 0 {
            return 0;
        }
        self.data_len / frame
    }

    pub(crate) fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.frame_count() as u64 * 1000 / self.sample_rate as u64
    }

    /// Maps the header onto an audio-service sample format; None when the
    /// encoding is outside the honest support set. G.711 code bytes are
    /// not raw PCM — the codec registry owns those tags.
    pub(crate) fn sample_format(&self) -> Option<AudioSampleFormat> {
        if matches!(self.format_tag, 6 | 7) {
            return None;
        }
        match (self.is_float, self.bits_per_sample) {
            (false, 8) => Some(AudioSampleFormat::U8),
            (false, 16) => Some(AudioSampleFormat::S16Le),
            (false, 32) => Some(AudioSampleFormat::S32Le),
            (true, 32) => Some(AudioSampleFormat::F32Le),
            _ => None,
        }
    }
}

/// Byte width of one sample for a stream format.
pub(crate) fn sample_width(format: AudioSampleFormat) -> usize {
    match format {
        AudioSampleFormat::U8 => 1,
        AudioSampleFormat::S16Le => 2,
        AudioSampleFormat::S32Le | AudioSampleFormat::F32Le => 4,
    }
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let hi = *bytes.get(offset + 1)?;
    let lo = *bytes.get(offset)?;
    Some(u16::from(lo) | (u16::from(hi) << 8))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let b0 = u32::from(*bytes.get(offset)?);
    let b1 = u32::from(*bytes.get(offset + 1)?);
    let b2 = u32::from(*bytes.get(offset + 2)?);
    let b3 = u32::from(*bytes.get(offset + 3)?);
    Some(b0 | (b1 << 8) | (b2 << 16) | (b3 << 24))
}

fn tag_at(bytes: &[u8], offset: usize, tag: &[u8; 4]) -> bool {
    bytes.get(offset..offset + 4) == Some(tag.as_slice())
}

/// Parses a RIFF/WAVE header out of `bytes`. Returns None for malformed
/// headers, unsupported encodings, or missing data chunks.
pub(crate) fn parse_wav(bytes: &[u8]) -> Option<WavInfo> {
    if bytes.len() < 12 || !tag_at(bytes, 0, b"RIFF") || !tag_at(bytes, 8, b"WAVE") {
        return None;
    }
    let mut info = WavInfo {
        channels: 0,
        sample_rate: 0,
        bits_per_sample: 0,
        is_float: false,
        format_tag: 0,
        block_align: 0,
        samples_per_block: 0,
        data_offset: 0,
        data_len: 0,
    };
    let mut have_fmt = false;
    let mut cursor = 12usize;
    while cursor + 8 <= bytes.len() {
        let chunk_len = le_u32(bytes, cursor + 4)? as usize;
        let body = cursor + 8;
        if tag_at(bytes, cursor, b"fmt ") {
            if chunk_len < 16 || body + 16 > bytes.len() {
                return None;
            }
            let format_tag = le_u16(bytes, body)?;
            let channels = u32::from(le_u16(bytes, body + 2)?);
            let rate = le_u32(bytes, body + 4)?;
            let block_align = u32::from(le_u16(bytes, body + 12)?);
            let bits = u32::from(le_u16(bytes, body + 14)?);
            match format_tag {
                1 => {
                    info.is_float = false;
                    if !matches!(bits, 8 | 16 | 32) {
                        return None;
                    }
                }
                3 => {
                    info.is_float = true;
                    if bits != 32 {
                        return None;
                    }
                }
                6 | 7 => {
                    // G.711 A-law / mu-law: code bytes are always 8 bits.
                    if bits != 8 {
                        return None;
                    }
                }
                0x11 => {
                    // IMA ADPCM: header needs the two extension fields,
                    // 4-bit encoding, and a roomy per-channel block.
                    if chunk_len < 20 || bits != 4 {
                        return None;
                    }
                    if block_align < channels * 4 || block_align % 4 != 0 {
                        return None;
                    }
                }
                _ => return None,
            }
            if (channels != 1 && channels != 2) || rate == 0 {
                return None;
            }
            info.channels = channels;
            info.sample_rate = rate;
            info.format_tag = format_tag;
            info.block_align = block_align;
            info.samples_per_block = u32::from(le_u16(bytes, body + 18).unwrap_or(0));
            info.bits_per_sample = bits;
            have_fmt = true;
        } else if tag_at(bytes, cursor, b"data") {
            let available = bytes.len().saturating_sub(body);
            info.data_offset = body;
            info.data_len = chunk_len.min(available);
        }
        // Chunk bodies are word-aligned in RIFF.
        cursor = body + chunk_len + (chunk_len & 1);
    }
    if !have_fmt || info.data_len == 0 || info.data_offset == 0 {
        return None;
    }
    Some(info)
}

/// Cheap container sniff: verifies the RIFF/WAVE magic and returns the
/// fmt-chunk encoding tag without full validation. Registry input.
pub(crate) fn fmt_tag_of(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 12 || !tag_at(bytes, 0, b"RIFF") || !tag_at(bytes, 8, b"WAVE") {
        return None;
    }
    let mut cursor = 12usize;
    while cursor + 8 <= bytes.len() {
        let chunk_len = le_u32(bytes, cursor + 4)? as usize;
        let body = cursor + 8;
        if tag_at(bytes, cursor, b"fmt ") {
            return le_u16(bytes, body);
        }
        cursor = body + chunk_len + (chunk_len & 1);
    }
    None
}

/// Converts interleaved raw samples starting at byte `start` into
/// normalized f32 samples written to `out`; returns converted sample count.
pub(crate) fn decode_samples(
    bytes: &[u8],
    start: usize,
    count: usize,
    format: AudioSampleFormat,
    out: &mut [f32],
) -> usize {
    let width = match format {
        AudioSampleFormat::U8 => 1usize,
        AudioSampleFormat::S16Le => 2,
        AudioSampleFormat::S32Le | AudioSampleFormat::F32Le => 4,
    };
    let mut decoded = 0usize;
    while decoded < count && decoded < out.len() {
        let offset = start + decoded * width;
        let Some(slice) = bytes.get(offset..offset + width) else {
            break;
        };
        let mut raw = [0u8; 4];
        raw[..width].copy_from_slice(slice);
        let word = u32::from(raw[0])
            | (u32::from(raw[1]) << 8)
            | (u32::from(raw[2]) << 16)
            | (u32::from(raw[3]) << 24);
        out[decoded] = match format {
            AudioSampleFormat::U8 => (f32::from(raw[0]) - 128.0) / 128.0,
            AudioSampleFormat::S16Le => f32::from(word as u16 as i16) / 32768.0,
            AudioSampleFormat::S32Le => (word as i32) as f32 / 2147483648.0,
            AudioSampleFormat::F32Le => f32::from_bits(word),
        };
        decoded += 1;
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn riff(frames: &[u8], fmt_body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let riff_len = 4 + 8 + fmt_body.len() + 8 + frames.len();
        out.extend_from_slice(&(riff_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt_body.len() as u32).to_le_bytes());
        out.extend_from_slice(fmt_body);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
        out.extend_from_slice(frames);
        out
    }

    fn pcm_fmt(channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        body.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        body.extend_from_slice(&bits.to_le_bytes());
        body
    }

    #[test]
    fn parses_pcm16_stereo_header_and_data_bounds() {
        let frames = [0u8; 48];
        let file = riff(&frames, &pcm_fmt(2, 48000, 16));
        let info = parse_wav(&file).expect("valid wav");
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.bits_per_sample, 16);
        assert!(!info.is_float);
        assert_eq!(info.frame_count(), 12);
        assert_eq!(info.duration_ms(), 0);
        assert_eq!(info.sample_format(), Some(AudioSampleFormat::S16Le));
        // Declared data length larger than the buffer clamps to what exists.
        let mut truncated = file.clone();
        truncated.truncate(file.len() - 8);
        let info = parse_wav(&truncated).expect("clamped wav");
        assert_eq!(info.frame_count(), 10);
    }

    #[test]
    fn accepts_supported_encodings_and_rejects_others() {
        let mono = riff(&[0u8; 8], &pcm_fmt(1, 44100, 8));
        let info = parse_wav(&mono).expect("pcm8");
        assert_eq!(info.sample_format(), Some(AudioSampleFormat::U8));

        let mut float_body = pcm_fmt(2, 48000, 32);
        float_body[0] = 3;
        let float = riff(&[0u8; 16], &float_body);
        let info = parse_wav(&float).expect("float32");
        assert_eq!(info.sample_format(), Some(AudioSampleFormat::F32Le));

        // 24-bit PCM has no audio-service mapping yet.
        let odd = riff(&[0u8; 9], &pcm_fmt(1, 44100, 24));
        assert_eq!(parse_wav(&odd), None);

        // ADPCM (tag 0x11) is now registry-supported; header sanity holds.
        let mut adpcm_body = Vec::new();
        adpcm_body.extend_from_slice(&0x11u16.to_le_bytes());
        adpcm_body.extend_from_slice(&2u16.to_le_bytes());
        adpcm_body.extend_from_slice(&44100u32.to_le_bytes());
        adpcm_body.extend_from_slice(&(88200u32).to_le_bytes());
        adpcm_body.extend_from_slice(&512u16.to_le_bytes()); // block align
        adpcm_body.extend_from_slice(&4u16.to_le_bytes());
        adpcm_body.extend_from_slice(&2u16.to_le_bytes());
        adpcm_body.extend_from_slice(&1017u16.to_le_bytes());
        let adpcm_file = riff(&[0u8; 1024], &adpcm_body);
        let info = parse_wav(&adpcm_file).expect("ima wav parses");
        assert_eq!(info.format_tag, 0x11);
        assert_eq!(info.block_align, 512);

        let mut bad_bits = adpcm_body.clone();
        bad_bits[14] = 16;
        assert_eq!(parse_wav(&riff(&[0u8; 8], &bad_bits)), None);

        let mut small_block = adpcm_body.clone();
        small_block[12] = 4;
        small_block[13] = 0;
        assert_eq!(parse_wav(&riff(&[0u8; 64], &small_block)), None);

        assert_eq!(parse_wav(b"not a wave"), None);
        assert_eq!(parse_wav(&[]), None);

        // Zero rate and bad channel counts are rejected.
        let zero_rate = riff(&[0u8; 4], &pcm_fmt(1, 0, 16));
        assert_eq!(parse_wav(&zero_rate), None);
        let three_ch = riff(&[0u8; 6], &pcm_fmt(3, 44100, 16));
        assert_eq!(parse_wav(&three_ch), None);
    }

    #[test]
    fn decode_samples_normalizes_each_format() {
        let mut out = [0f32; 4];

        let s16bytes = 0i16.to_le_bytes();
        let neg = (-16384i16).to_le_bytes();
        let mut input = [0u8; 4];
        input[..2].copy_from_slice(&neg);
        input[2..].copy_from_slice(&s16bytes);
        assert_eq!(
            decode_samples(&input, 0, 2, AudioSampleFormat::S16Le, &mut out),
            2
        );
        assert!((out[0] + 0.5).abs() < 1e-6);
        assert_eq!(out[1], 0.0);

        let u8mid = [128u8, 255];
        assert_eq!(
            decode_samples(&u8mid, 0, 2, AudioSampleFormat::U8, &mut out),
            2
        );
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 127.0 / 128.0).abs() < 1e-6);

        let f32s = (0.25f32).to_le_bytes();
        let mut fin = [0u8; 4];
        fin.copy_from_slice(&f32s);
        assert_eq!(
            decode_samples(&fin, 0, 1, AudioSampleFormat::F32Le, &mut out),
            1
        );
        assert!((out[0] - 0.25).abs() < 1e-6);

        // Reads past the end stop cleanly instead of panicking.
        assert_eq!(
            decode_samples(&u8mid, 1, 4, AudioSampleFormat::U8, &mut out),
            1
        );
    }

    fn g711_fmt(tag: u16, channels: u16, rate: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&tag.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&(rate * u32::from(channels)).to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&8u16.to_le_bytes());
        body
    }

    #[test]
    fn parses_g711_headers_and_rejects_wrong_widths() {
        let file = riff(&[0u8; 6], &g711_fmt(7, 2, 8000));
        let info = parse_wav(&file).expect("mulaw wav parses");
        assert_eq!(info.format_tag, 7);
        assert_eq!(info.frame_bytes(), 2);
        assert_eq!(info.frame_count(), 3);
        // G.711 is not a service sample format; the codec registry owns it.
        assert_eq!(info.sample_format(), None);

        let alaw = riff(&[0u8; 3], &g711_fmt(6, 1, 8000));
        let info = parse_wav(&alaw).expect("alaw wav parses");
        assert_eq!(info.format_tag, 6);
        assert_eq!(info.frame_count(), 3);

        let mut wide = g711_fmt(7, 1, 8000);
        wide[14] = 16; // bits per sample must stay 8 for G.711
        assert_eq!(parse_wav(&riff(&[0u8; 2], &wide)), None);
    }
}
