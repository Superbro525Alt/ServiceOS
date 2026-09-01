use serviceos_userspace_runtime::AudioSampleFormat;

/// Pluggable decode pipeline. Container sniffing happens on the file
/// header magic (RIFF/WAVE), then a static registry maps the fmt-chunk
/// encoding tag onto a decoder: PCM passthrough variants, per-byte
/// G.711 A-law/mu-law (tags 6/7), plus a real block-based IMA-ADPCM
/// decoder. Decoders produce normalized interleaved f32 frames;
/// anything outside the registry fails honestly.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecoderKind {
    Pcm(AudioSampleFormat),
    MuLaw,
    ALaw,
    ImaAdpcm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodecError {
    /// Header does not sniff as a known container.
    NotWav,
    /// Known container but the encoding has no registered decoder.
    UnsupportedEncoding,
    /// Encoding is registered but the header violates its constraints.
    BadHeader,
}

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Sequential frame decoder over the raw file bytes. Copy so playback
/// state can park inside MediaState without borrowing hazards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Decoder {
    pub(crate) channels: u32,
    pub(crate) sample_rate: u32,
    kind: DecoderKind,
    data_offset: usize,
    data_len: usize,
    /// Total frames the header promises (used for the progress meter).
    promised_frames: usize,
    // PCM decode position (bytes into the data chunk).
    pcm_cursor: usize,
    // IMA-ADPCM running state.
    data_cursor: usize,
    pred: [i32; 2],
    index: [i8; 2],
    block_align: usize,
    samples_per_block: usize,
    /// True while parked mid-block waiting for more output space.
    block_pending: bool,
    /// Samples decoded per channel within the pending block.
    tick: [usize; 2],
    /// Frames to decode-and-drop after a seek before emitting again.
    discard_pending: usize,
    /// wSamplesPerBlock came from the header (not the block_align
    /// formula), so frame→block arithmetic is trustworthy.
    spb_from_header: bool,
}

impl Decoder {
    pub(crate) fn total_frames(&self) -> usize {
        self.promised_frames
    }

    pub(crate) fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.promised_frames as u64 * 1000 / self.sample_rate as u64
    }

    pub(crate) fn kind_of(&self) -> &DecoderKind {
        &self.kind
    }

    /// Sniffs the container magic, consults the registry, and builds a
    /// fresh decoder positioned at the first data byte.
    pub(crate) fn open(bytes: &[u8]) -> Result<Self, CodecError> {
        let tag = crate::wav::fmt_tag_of(bytes).ok_or(CodecError::NotWav)?;
        let kind = match tag {
            1 | 3 => DecoderKind::Pcm(
                crate::wav::parse_wav(bytes)
                    .and_then(|info| info.sample_format())
                    .ok_or(CodecError::BadHeader)?,
            ),
            7 => DecoderKind::MuLaw,
            6 => DecoderKind::ALaw,
            0x11 => DecoderKind::ImaAdpcm,
            _ => return Err(CodecError::UnsupportedEncoding),
        };
        let info = crate::wav::parse_wav(bytes).ok_or(CodecError::BadHeader)?;
        let channels = info.channels as usize;
        let (promised_frames, block_align, samples_per_block, spb_from_header) = match kind {
            DecoderKind::Pcm(_) | DecoderKind::MuLaw | DecoderKind::ALaw => {
                (info.frame_count(), 0usize, 0usize, true)
            }
            DecoderKind::ImaAdpcm => {
                let block_align = info.block_align as usize;
                // ((blockAlign - 4*ch) * 8) / (4 bits * ch) + 1 per spec.
                let formula = (block_align - channels * 4) * 8 / (4 * channels.max(1)) + 1;
                let hint = info.samples_per_block as usize;
                // Header hint wins when present and physically plausible.
                let (spb, hinted) = if hint > 1 && hint <= formula {
                    (hint, true)
                } else if hint == 0 || hint > formula {
                    (formula, false)
                } else {
                    (hint, true)
                };
                (
                    info.data_len.div_ceil(block_align) * spb,
                    block_align,
                    spb.max(1),
                    hinted,
                )
            }
        };
        Ok(Self {
            channels: info.channels,
            sample_rate: info.sample_rate,
            kind,
            data_offset: info.data_offset,
            data_len: info.data_len,
            promised_frames,
            pcm_cursor: 0,
            data_cursor: 0,
            pred: [0; 2],
            index: [0; 2],
            block_align,
            samples_per_block,
            block_pending: false,
            tick: [0; 2],
            discard_pending: 0,
            spb_from_header,
        })
    }

    /// Pulls up to `max_frames` frames into `out` continuing from wherever
    /// the previous call stopped. Returns the number of complete frames
    /// written. Zero means exhausted or truncated input.
    pub(crate) fn decode_next(
        &mut self,
        bytes: &[u8],
        max_frames: usize,
        out: &mut [f32],
    ) -> usize {
        if max_frames == 0 || out.is_empty() || self.data_len == 0 {
            return 0;
        }
        let data_end = self.data_offset + self.data_len;
        let channels = self.channels as usize;
        let frames_cap = (out.len() / channels.max(1)).min(max_frames);
        match self.kind {
            DecoderKind::Pcm(format) => {
                let target = frames_cap * channels;
                let written = crate::wav::decode_samples(
                    bytes,
                    self.data_offset + self.pcm_cursor,
                    target,
                    format,
                    &mut out[..target],
                );
                self.pcm_cursor += written * crate::wav::sample_width(format);
                written / channels.max(1)
            }
            DecoderKind::MuLaw => {
                let target = frames_cap * channels;
                let written = decode_g711(
                    bytes,
                    self.data_offset + self.pcm_cursor,
                    target,
                    out,
                    mulaw_to_linear,
                );
                self.pcm_cursor += written;
                written / channels.max(1)
            }
            DecoderKind::ALaw => {
                let target = frames_cap * channels;
                let written = decode_g711(
                    bytes,
                    self.data_offset + self.pcm_cursor,
                    target,
                    out,
                    alaw_to_linear,
                );
                self.pcm_cursor += written;
                written / channels.max(1)
            }
            DecoderKind::ImaAdpcm => self.decode_adpcm(bytes, data_end, frames_cap, out),
        }
    }

    /// Repositions the decode cursor to `target_frame`. Per-sample
    /// encodings (PCM, G.711) are byte-aligned so landing frames are
    /// exact. Block-compressed IMA lands on the boundary of the block
    /// at-or-before the target: the decoder re-seeds from that block's
    /// per-channel header and, when the header's wSamplesPerBlock is
    /// trustworthy, drops frames inside the block to hit the exact
    /// target on the next decode. Without a usable wSamplesPerBlock the
    /// landing is block-granular (no intra-block discard) because the
    /// frame↔byte map cannot be trusted. A target at-or-past the last
    /// block parks the cursor at end-of-stream for a clean finish.
    pub(crate) fn seek_frames(&mut self, target_frame: usize) -> bool {
        match self.kind {
            DecoderKind::Pcm(format) => {
                let frame_bytes = self.channels as usize * crate::wav::sample_width(format);
                self.pcm_cursor = target_frame.min(self.promised_frames) * frame_bytes;
            }
            DecoderKind::MuLaw | DecoderKind::ALaw => {
                self.pcm_cursor = target_frame.min(self.promised_frames) * self.channels as usize;
            }
            DecoderKind::ImaAdpcm => {
                let target = target_frame.min(self.promised_frames);
                let (byte_offset, intra, past_end) = ima_seek_math(
                    target,
                    self.samples_per_block,
                    self.block_align,
                    self.data_len,
                );
                self.block_pending = false;
                self.tick = [0; 2];
                self.pred = [0; 2];
                self.index = [0; 2];
                if past_end {
                    self.data_cursor = self.data_len;
                    self.discard_pending = 0;
                } else {
                    self.data_cursor = byte_offset;
                    // Intra-block discard only when the frame→block map
                    // came from the header; otherwise the landing is the
                    // block boundary itself (documented granularity).
                    self.discard_pending = if self.spb_from_header { intra } else { 0 };
                }
            }
        }
        true
    }

    /// Sequential IMA ADPCM decode following the Microsoft WAV layout:
    /// per-block 4-byte headers (predictor, step index, reserved), then
    /// 32-bit sub-blocks interleaved per channel, low nibble first. The
    /// block's initial predictor is emitted as the first sample frame.
    /// A mid-block output cap parks state so the next call resumes in
    /// place, making chunked and single-shot decodes identical.
    fn decode_adpcm(
        &mut self,
        bytes: &[u8],
        data_end: usize,
        frames_cap: usize,
        out: &mut [f32],
    ) -> usize {
        let ch = self.channels as usize;
        // A seek landed on a block boundary and must drop `discard_pending`
        // frames inside it before output resumes; decoding them into a
        // scratch buffer re-seeds the predictor/step-index chain honestly.
        // Truncated data ends the discard early and the stream reads as
        // exhausted from there.
        if self.discard_pending > 0 {
            let mut left = self.discard_pending;
            self.discard_pending = 0;
            let mut scratch = [0f32; 256];
            while left > 0 {
                let cap = left.min(scratch.len() / ch.max(1));
                let dropped = self.decode_adpcm(bytes, data_end, cap, &mut scratch[..cap * ch]);
                if dropped == 0 {
                    break;
                }
                left -= dropped;
            }
        }
        let target_samples = frames_cap * ch;
        let mut written_samples = 0usize;
        // Address everything relative to the data chunk.
        let block_count_max = if self.data_offset <= data_end && self.data_offset <= bytes.len() {
            data_end.min(bytes.len())
        } else {
            return 0;
        };
        let data = &bytes[self.data_offset..block_count_max];
        let local_end = data.len();
        while written_samples < target_samples && self.data_cursor + ch * 4 <= local_end {
            let block_base = self.data_cursor;
            if !self.block_pending {
                for c in 0..ch {
                    let h = block_base + c * 4;
                    let lo = i16::from(data[h]);
                    let hi = i16::from(data[h + 1]);
                    self.pred[c] = i32::from(lo | (hi << 8));
                    self.index[c] = (data[h + 2] as i8).clamp(0, 88);
                    self.tick[c] = 0;
                }
                // Initial predictor counts as the first sample frame.
                if written_samples < target_samples {
                    for c in 0..ch {
                        out[written_samples] = self.pred[c] as f32 / 32768.0;
                        written_samples += 1;
                    }
                }
                for c in 0..ch {
                    self.tick[c] += 1;
                }
            }
            self.block_pending = false;

            let payload_start = block_base + ch * 4;
            let block_limit = (block_base + self.block_align).min(local_end);
            let avail = block_limit.saturating_sub(payload_start);
            let sub = ch * 4;
            // SDL-consistent truncation rule: whole interleaved sub-blocks
            // are guaranteed; a partial tail yields (rem % 4) * 2 samples.
            let budget = if avail >= sub {
                self.samples_per_block - 1
            } else {
                (avail / sub) * 8 + if avail > sub - 4 { (avail % 4) * 2 } else { 0 }
            };
            loop {
                if self.tick[0].saturating_sub(1) >= budget {
                    break;
                }
                if written_samples >= target_samples {
                    // Parked mid-block; resume here on the next call.
                    self.block_pending = true;
                    break;
                }
                for c in 0..ch {
                    if self.tick[c].saturating_sub(1) >= budget {
                        continue;
                    }
                    let ps = self.tick[c] - 1; // payload-relative sample no.
                    let t = ps >> 1;
                    let addr = if ch == 1 {
                        payload_start + t
                    } else {
                        payload_start + (t / 4) * 8 + c * 4 + (t % 4)
                    };
                    let byte = if addr < block_limit { data[addr] } else { 0 };
                    let code = usize::from(if ps & 1 == 0 { byte & 0xf } else { byte >> 4 });
                    let sample = ima_step(self.pred[c], self.index[c], code);
                    self.pred[c] = sample.0;
                    self.index[c] = sample.1;
                    out[written_samples] = self.pred[c] as f32 / 32768.0;
                    written_samples += 1;
                    self.tick[c] += 1;
                }
            }
            if !self.block_pending {
                self.data_cursor = block_base + self.block_align.min(block_limit - block_base);
            }
        }
        let _ = block_count_max;
        written_samples / ch
    }
}

/// Canonical G.711 mu-law (tag 7) expansion, Sun g711.c form: complement
/// the code byte, split 3-bit segment exponent and 4-bit mantissa, add the
/// 0x84 bias and shift by the segment. Golden anchors: 0x00 -> -32124,
/// 0x80 -> +32124, 0xFF/0x7F -> 0.
pub(crate) fn mulaw_to_linear(byte: u8) -> i16 {
    let value = !byte;
    let mut t = (i32::from(value & 0x0F) << 3) + 0x84;
    t <<= u32::from((value >> 4) & 0x07);
    let linear = if value & 0x80 != 0 {
        0x84 - t
    } else {
        t - 0x84
    };
    linear as i16
}

/// Canonical G.711 A-law (tag 6) expansion, Sun g711.c form: XOR the
/// alternate-bit mask, 3-bit segment, 4-bit mantissa with per-segment
/// offsets. Golden anchors: 0x00 -> -5504, 0xAA -> +32256, 0xD5 -> +8,
/// 0x55 -> -8.
pub(crate) fn alaw_to_linear(byte: u8) -> i16 {
    let value = byte ^ 0x55;
    let segment = (value >> 4) & 0x07;
    let mut t = i32::from(value & 0x0F) << 4;
    match segment {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= segment - 1;
        }
    }
    let linear = if value & 0x80 != 0 { t } else { -t };
    linear as i16
}

/// Per-byte G.711 decode: converts up to `target_samples` code bytes
/// starting at `start` into normalized f32 samples. Stops cleanly at the
/// end of input. Returns the number of samples written.
fn decode_g711(
    bytes: &[u8],
    start: usize,
    target_samples: usize,
    out: &mut [f32],
    expand: fn(u8) -> i16,
) -> usize {
    let mut decoded = 0usize;
    while decoded < target_samples && decoded < out.len() {
        let Some(byte) = bytes.get(start + decoded) else {
            break;
        };
        out[decoded] = f32::from(expand(*byte)) / 32768.0;
        decoded += 1;
    }
    decoded
}

/// Pure seek arithmetic shared by the key bindings: `delta_secs` may be
/// negative; the landing frame clamps to [0, total]. A zero sample rate
/// (degenerate header) never moves.
pub(crate) fn seek_target_frame(
    current_frame: usize,
    delta_secs: i32,
    sample_rate: u32,
    total_frames: usize,
) -> usize {
    if sample_rate == 0 {
        return 0;
    }
    let target = current_frame as i64 + i64::from(delta_secs) * i64::from(sample_rate);
    target.clamp(0, total_frames as i64) as usize
}

/// Frame→block seek arithmetic for IMA-ADPCM. Block `b` holds frames
/// `[b*spb, (b+1)*spb)` with the block's initial predictor emitted as
/// its first frame, so the landing byte is `target/spb * blockAlign`
/// and `target%spb` frames must be dropped inside that block. Returns
/// `(byte_offset, intra_block_frames, past_end)`; `past_end` means the
/// landing byte sits at-or-beyond the data chunk and the stream should
/// park at end-of-data for a clean finish.
fn ima_seek_math(
    target: usize,
    samples_per_block: usize,
    block_align: usize,
    data_len: usize,
) -> (usize, usize, bool) {
    let spb = samples_per_block.max(1);
    let block = target / spb;
    let byte_offset = block.saturating_mul(block_align.max(1));
    (byte_offset, target % spb, byte_offset >= data_len)
}

/// One IMA nibble: returns (new predictor, new index) per the spec math.
fn ima_step(mut pred: i32, mut index: i8, code: usize) -> (i32, i8) {
    let step = STEP_TABLE[index.clamp(0, 88) as usize];
    let mut diff = step >> 3;
    if code & 4 != 0 {
        diff += step >> 2;
    }
    if code & 2 != 0 {
        diff += step >> 1;
    }
    if code & 1 != 0 {
        diff += step;
    }
    pred = if code & 8 != 0 {
        pred - diff
    } else {
        pred + diff
    };
    pred = pred.clamp(-32768, 32767);
    index = (index + INDEX_TABLE[code]).clamp(0, 88);
    (pred, index)
}

// ============================================================
// Host tests (spec-computed vectors, no fixtures copied in).
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn riff(data: &[u8], fmt_body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let riff_len = 4 + 8 + fmt_body.len() + 8 + data.len();
        out.extend_from_slice(&(riff_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt_body.len() as u32).to_le_bytes());
        out.extend_from_slice(fmt_body);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn pcm_fmt(channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate * u32::from(channels) * u32::from(bits) / 8;
        body.extend_from_slice(&byte_rate.to_le_bytes());
        body.extend_from_slice(&((channels * bits / 8) as u16).to_le_bytes());
        body.extend_from_slice(&bits.to_le_bytes());
        body
    }

    fn ima_fmt(channels: u16, rate: u32, block_align: u16) -> Vec<u8> {
        let spb = if block_align >= channels * 4 {
            ((u32::from(block_align) - u32::from(channels) * 4) * 8 / (4 * u32::from(channels)) + 1)
                as u16
        } else {
            0
        };
        ima_fmt_hint(channels, rate, block_align, spb)
    }

    /// IMA fmt chunk with an explicit wSamplesPerBlock (0 = absent, the
    /// malformed-header fallback case).
    fn ima_fmt_hint(channels: u16, rate: u32, block_align: u16, spb: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0x11u16.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate * u32::from(block_align);
        body.extend_from_slice(&byte_rate.to_le_bytes());
        body.extend_from_slice(&block_align.to_le_bytes());
        body.extend_from_slice(&4u16.to_le_bytes());
        body.extend_from_slice(&2u16.to_le_bytes()); // cbSize
        body.extend_from_slice(&spb.to_le_bytes()); // wSamplesPerBlock
        body
    }

    /// Mono IMA block: 4-byte header then `payload`.
    fn mono_block(pred: i16, index: u8, payload: &[u8]) -> Vec<u8> {
        let mut block = Vec::new();
        block.extend_from_slice(&pred.to_le_bytes());
        block.push(index);
        block.push(0);
        block.extend_from_slice(payload);
        block
    }

    fn stereo_block(p0: i16, i0: u8, pay0: &[u8], p1: i16, i1: u8, pay1: &[u8]) -> Vec<u8> {
        let mut block = Vec::new();
        block.extend_from_slice(&p0.to_le_bytes());
        block.push(i0);
        block.push(0);
        block.extend_from_slice(&p1.to_le_bytes());
        block.push(i1);
        block.push(0);
        assert_eq!(pay0.len(), pay1.len());
        // Microsoft layout: each channel's 32-bit sub-block is stored
        // contiguously before the next channel's.
        for pay in [pay0, pay1] {
            let mut chunk = pay.to_vec();
            while chunk.len() % 4 != 0 {
                chunk.push(0);
            }
            block.extend_from_slice(&chunk);
        }
        block
    }

    #[test]
    fn ima_mono_block_decodes_reference_vectors() {
        // Hand-computed against the IMA specification tables:
        // step(0)=7; nibble 0 keeps predictor 0; nibble 7 adds
        // (7>>2)+(7>>1)+7+(7>>3) = 11. The block's initial predictor is
        // itself the first emitted sample per the MS spec.
        let payload = [0x70u8, 0x00, 0x00, 0x00];
        let data = mono_block(0, 0, &payload);
        let file = riff(&data, &ima_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("ima wav opens");
        assert_eq!(dec.kind_of(), &DecoderKind::ImaAdpcm);
        assert_eq!(dec.channels, 1);
        let mut out = [0f32; 16];
        let samples = dec.decode_next(&file, 16, &mut out);
        assert_eq!(samples, 9);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert!((out[2] - 11.0 / 32768.0).abs() < 1e-9);

        // Second vector: step(20)=50 negative motion then heavy positive.
        // nibble 8 => 2000-6=1994; nibble f => 1994-(11+22+45+5)=1911.
        let payload = [0xf8u8, 0x00, 0x00, 0x00];
        let data = mono_block(2000, 20, &payload);
        let file = riff(&data, &ima_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("ima wav opens");
        let mut out = [0f32; 16];
        let samples = dec.decode_next(&file, 16, &mut out);
        assert_eq!(samples, 9);
        assert_eq!(out[0], 2000.0 / 32768.0);
        assert_eq!(out[1], 1994.0 / 32768.0);
        assert_eq!(out[2], 1911.0 / 32768.0);
    }

    #[test]
    fn ima_stereo_channels_decode_independently() {
        // 4-byte payloads per channel (one full interleaved sub-block).
        // ch1 vector: step(2)=9; nibble 7 gives 100+16=116 with index->10,
        // nibble 7 again at step(19)=45 gives 116-34=82? No: positive code
        // adds: 116+34=150.
        let data = stereo_block(0, 0, &[0x70u8, 0, 0, 0], 100, 2, &[0x77u8, 0, 0, 0]);
        let file = riff(&data, &ima_fmt(2, 16000, 16));
        let mut dec = Decoder::open(&file).expect("stereo ima wav opens");
        let mut out = [0f32; 64];
        let frames = dec.decode_next(&file, 16, &mut out);
        assert_eq!(frames, 9); // header frame + one full sub-block per ch
        let want = [
            0.0,
            100.0 / 32768.0,
            0.0,
            116.0 / 32768.0,
            11.0 / 32768.0,
            150.0 / 32768.0,
        ];
        assert_eq!(&out[..6], &want);
    }

    #[test]
    fn ima_multi_chunk_sequential_decode_matches_single_shot() {
        // 20 mono blocks of identical shape decoded either all-at-once or
        // through 5-sample caps must converge bit-for-bit.
        let payload = [0x21u8, 0x43, 0x65, 0x87];
        let mut data = Vec::new();
        for round in 0..20u16 {
            let mut block = mono_block(
                i16::try_from(100 * i32::from(round)).unwrap_or(30000),
                3,
                &payload,
            );
            data.append(&mut block);
        }
        let file = riff(&data, &ima_fmt(1, 8000, 8));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full_out = [0f32; 400];
        let got_full = whole.decode_next(&file, 200, &mut full_out);

        let mut pieced = Decoder::open(&file).expect("opens");
        let mut piece_out = [0f32; 400];
        let mut filled = 0usize;
        while filled < 200 {
            let view = &mut piece_out[filled..filled + 10];
            let wrote = pieced.decode_next(&file, 5, view);
            if wrote == 0 {
                break;
            }
            filled += wrote;
        }
        assert_eq!(got_full * 1, filled);
        assert_eq!(&full_out[..filled], &piece_out[..filled]);
    }

    #[test]
    fn pcm_kinds_flow_through_registry_with_old_normalization() {
        // 8-bit unsigned
        let data = [128u8, 255, 0];
        let file = riff(&data, &pcm_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("pcm8 opens");
        assert_eq!(dec.kind_of(), &DecoderKind::Pcm(AudioSampleFormat::U8));
        let mut out = [0f32; 4];
        let frames = dec.decode_next(&file, 3, &mut out);
        assert_eq!(frames, 3);
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 127.0 / 128.0).abs() < 1e-6);
        assert!((out[2] + 1.0).abs() < 1e-6);

        // s16 passthrough across a chunk boundary
        let mut s16 = Vec::new();
        s16.extend_from_slice(&0i16.to_le_bytes());
        s16.extend_from_slice(&(-16384i16).to_le_bytes());
        let file = riff(&s16, &pcm_fmt(2, 48000, 16));
        let mut dec = Decoder::open(&file).expect("pcm16 opens");
        let mut out = [0f32; 2];
        assert_eq!(dec.decode_next(&file, 1, &mut out), 1);
        assert_eq!(out[0], 0.0);
        assert!((out[1] + 0.5).abs() < 1e-6);
        // Stream exhausted: nothing more to pull, buffer untouched.
        assert_eq!(dec.decode_next(&file, 1, &mut out), 0);
        assert!((out[1] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn sniff_dispatch_and_error_paths() {
        // Non-WAV magic refuses cleanly.
        assert_eq!(Decoder::open(b"not a wave"), Err(CodecError::NotWav));

        // Registered encodings pick their decoder kind.
        let pcm_file = riff(&[0u8; 4], &pcm_fmt(1, 44100, 16));
        let dec = Decoder::open(&pcm_file).expect("pcm opens");
        assert_eq!(dec.kind_of(), &DecoderKind::Pcm(AudioSampleFormat::S16Le));
        assert_eq!(dec.total_frames(), 2);
        assert_eq!(dec.duration_ms(), 0);

        let ima_file = riff(&mono_block(0, 0, &[0x00; 4]), &ima_fmt(1, 8000, 8));
        let dec = Decoder::open(&ima_file).expect("ima opens");
        assert_eq!(dec.kind_of(), &DecoderKind::ImaAdpcm);
        assert_eq!(dec.total_frames(), 9);

        // MP3-in-WAV (tag 0x55) hits the registry miss notice.
        let mut mp3_body = pcm_fmt(1, 44100, 16);
        mp3_body[0] = 0x55;
        let mp3_file = riff(&[0u8; 4], &mp3_body);
        assert_eq!(
            Decoder::open(&mp3_file),
            Err(CodecError::UnsupportedEncoding)
        );

        // IMA header games: 24-bit IMA and undersized block align fail loud.
        let mut bits_bad = ima_fmt(1, 8000, 8);
        bits_bad[14] = 16;
        assert_eq!(
            Decoder::open(&riff(&mono_block(0, 0, &[0x00; 4]), &bits_bad)),
            Err(CodecError::BadHeader)
        );
        assert_eq!(
            Decoder::open(&riff(
                &stereo_block(0, 0, &[0x00; 4], 0, 0, &[0x00; 4]),
                &ima_fmt(2, 8000, 6)
            )),
            Err(CodecError::BadHeader)
        );

        // Minimal legal block: header-only frame at the smallest align.
        let file = riff(&mono_block(0, 0, &[]), &ima_fmt(1, 8000, 4));
        let mut dec = Decoder::open(&file).expect("header-only block opens");
        let mut out = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut out), 1);
        assert_eq!(out[0], 0.0);
        assert_eq!(dec.decode_next(&file, 4, &mut out), 0);

        // Truncated tail keeps whole guaranteed sub-blocks only.
        let mut cut = mono_block(0, 0, &[0x10; 4]);
        cut.truncate(6); // two payload bytes into an 8-byte block claim
        let file = riff(&cut, &ima_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("truncated block opens");
        let mut big = [0f32; 16];
        let got = dec.decode_next(&file, 16, &mut big);
        // Two payload bytes = four mono samples plus the header frame.
        assert_eq!(got, 5);
    }

    fn g711_fmt(tag: u16, channels: u16, rate: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&tag.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&(rate * u32::from(channels)).to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes()); // one byte per sample
        body.extend_from_slice(&8u16.to_le_bytes());
        body
    }

    #[test]
    fn seek_target_frame_moves_clamps_and_guards() {
        // +10s at 100 Hz from frame 1000 lands on frame 2000.
        assert_eq!(seek_target_frame(1000, 10, 100, 5000), 2000);
        // -10s clamps at the start.
        assert_eq!(seek_target_frame(100, -10, 100, 5000), 0);
        // +10s past the end clamps at the final frame.
        assert_eq!(seek_target_frame(4500, 10, 100, 5000), 5000);
        // Degenerate header (rate 0) never advances.
        assert_eq!(seek_target_frame(0, 5, 0, 7), 0);
    }

    #[test]
    fn pcm_seek_repositions_the_decode_stream() {
        let data: Vec<u8> = (10u8..=80).step_by(10).collect(); // 8 frames
        let file = riff(&data, &pcm_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("pcm opens");
        let mut out = [0f32; 2];
        assert_eq!(dec.decode_next(&file, 2, &mut out), 2);
        assert!(dec.seek_frames(5));
        let mut resumed = [0f32; 3];
        assert_eq!(dec.decode_next(&file, 3, &mut resumed), 3);
        assert!((resumed[0] - (60.0 - 128.0) / 128.0).abs() < 1e-6);
        assert!((resumed[1] - (70.0 - 128.0) / 128.0).abs() < 1e-6);
        assert!((resumed[2] - (80.0 - 128.0) / 128.0).abs() < 1e-6);
    }

    #[test]
    fn pcm_seek_clamps_beyond_the_final_frame() {
        let file = riff(&[10u8, 20, 30, 40], &pcm_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("pcm opens");
        assert!(dec.seek_frames(999));
        let mut out = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut out), 0);
    }

    #[test]
    fn ima_seek_frame_block_math_matrix() {
        // Mono, 8-byte blocks: 4-byte header + 4 payload bytes = 9 frames.
        // Stereo, 16-byte blocks: same 9 frames from two 4-byte payloads.
        // Mono, 12-byte blocks ("odd" two-sub-block payload): 17 frames.
        for (spb, align, ch, target, want_block, want_intra) in [
            (9usize, 8usize, 1usize, 0usize, 0usize, 0usize),
            (9, 8, 1, 8, 0, 8),
            (9, 8, 1, 9, 1, 0),
            (9, 8, 1, 44, 4, 8),
            (9, 16, 2, 12, 1, 3),
            (17, 12, 1, 17, 1, 0),
            (17, 12, 1, 25, 1, 8),
        ] {
            let (off, intra, past) = ima_seek_math(target, spb, align, align * 5);
            assert_eq!(off, want_block * align, "block byte offset t={target}");
            assert_eq!(intra, want_intra, "intra frames t={target}");
            assert!(!past, "in-range t={target} ch={ch}");
        }
        // Truncated final block: landing byte at-or-past data_len parks
        // at end-of-data (mono align 8, only 6 bytes present).
        let (off, intra, past) = ima_seek_math(9, 9, 8, 6);
        assert_eq!(off, 8);
        assert_eq!(intra, 0);
        assert!(past);
        // Last in-range block still lands even though data is short.
        let (off, intra, past) = ima_seek_math(5, 9, 8, 6);
        assert_eq!(off, 0);
        assert_eq!(intra, 5);
        assert!(!past);
    }

    #[test]
    fn ima_seek_re_seeds_from_block_header_exactly() {
        // Re-seed parity: decode starting at block N after a seek must be
        // bit-identical to decoding from 0 and discarding N*spb frames,
        // because the block header fully restores predictor/step-index.
        let payload = [0x21u8, 0x43, 0x65, 0x87];
        let mut data = Vec::new();
        for round in 0..6u16 {
            let mut block = mono_block(
                i16::try_from(300 * i32::from(round)).unwrap_or(30000),
                u8::try_from((2 + round) % 60).unwrap_or(0),
                &payload,
            );
            data.append(&mut block);
        }
        let file = riff(&data, &ima_fmt(1, 8000, 8));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full = [0f32; 64];
        let total = whole.decode_next(&file, 60, &mut full);
        assert_eq!(total, 54); // 6 blocks x 9 frames

        for boundary in [0usize, 9, 27] {
            let mut seeked = Decoder::open(&file).expect("opens");
            assert!(seeked.seek_frames(boundary));
            let mut got = [0f32; 64];
            let frames = seeked.decode_next(&file, 60, &mut got);
            assert_eq!(frames, total - boundary);
            assert_eq!(
                &got[..frames],
                &full[boundary..total],
                "boundary {boundary}"
            );
        }
    }

    #[test]
    fn ima_seek_intra_block_discard_lands_exactly() {
        // Targets strictly inside blocks: the seek drops frames within
        // the re-seeded block so output resumes on the exact frame.
        let payload = [0x91u8, 0xD2, 0x43, 0x65];
        let mut data = Vec::new();
        for round in 0..4u16 {
            let mut block = mono_block(
                i16::try_from(500 * i32::from(round) - 300).unwrap_or(30000),
                u8::try_from((5 * round + 3) % 60).unwrap_or(0),
                &payload,
            );
            data.append(&mut block);
        }
        let file = riff(&data, &ima_fmt(1, 8000, 8));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full = [0f32; 40];
        let total = whole.decode_next(&file, 40, &mut full);
        assert_eq!(total, 36);

        for target in [1usize, 4, 5, 10, 17, 35] {
            let mut seeked = Decoder::open(&file).expect("opens");
            assert!(seeked.seek_frames(target));
            let mut got = [0f32; 40];
            let frames = seeked.decode_next(&file, 40, &mut got);
            assert_eq!(frames, total - target, "target {target}");
            assert_eq!(&got[..frames], &full[target..total], "target {target}");
        }
    }

    #[test]
    fn ima_seek_stereo_discard_is_frame_exact() {
        // Stereo seek: both channels re-seed from their own 4-byte
        // headers and discard in per-frame lockstep.
        let pay0 = [0x70u8, 0x11, 0x22, 0x33];
        let pay1 = [0x8Fu8, 0x44, 0x55, 0x66];
        let mut data = Vec::new();
        for round in 0..3u16 {
            let mut block = stereo_block(
                i16::try_from(200 * i32::from(round)).unwrap_or(30000),
                u8::try_from((3 * round + 1) % 60).unwrap_or(0),
                &pay0,
                i16::try_from(-150 * i32::from(round)).unwrap_or(-30000),
                u8::try_from((7 * round + 5) % 60).unwrap_or(0),
                &pay1,
            );
            data.append(&mut block);
        }
        let file = riff(&data, &ima_fmt(2, 16000, 16));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full = [0f32; 64];
        let total = whole.decode_next(&file, 32, &mut full);
        assert_eq!(total, 27); // 3 blocks x 9 frames

        for target in [1usize, 5, 9, 12, 20] {
            let mut seeked = Decoder::open(&file).expect("opens");
            assert!(seeked.seek_frames(target));
            let mut got = [0f32; 64];
            let frames = seeked.decode_next(&file, 32, &mut got);
            assert_eq!(frames, total - target, "target {target}");
            assert_eq!(
                &got[..frames * 2],
                &full[target * 2..total * 2],
                "target {target}"
            );
        }
    }

    #[test]
    fn ima_seek_clamps_and_parks_cleanly_at_eof() {
        let file = riff(&mono_block(0, 0, &[0x00; 4]), &ima_fmt(1, 8000, 8));
        let mut dec = Decoder::open(&file).expect("ima opens");
        // Beyond the final frame clamps to end-of-stream: nothing more
        // decodes and the stream reads as finished.
        assert!(dec.seek_frames(999));
        let mut out = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut out), 0);
        // Exactly the final frame count is end-of-stream too.
        assert!(dec.seek_frames(9));
        assert_eq!(dec.decode_next(&file, 4, &mut out), 0);
        // Seeking back to 0 after EOF re-opens from the top.
        assert!(dec.seek_frames(0));
        let mut restart = [0f32; 2];
        assert_eq!(dec.decode_next(&file, 2, &mut restart), 2);
        assert_eq!(restart[0], 0.0);
    }

    #[test]
    fn ima_seek_without_header_hint_lands_on_block_boundary() {
        // wSamplesPerBlock absent (0): the frame->block map cannot be
        // trusted for intra-block arithmetic, so the seek lands on the
        // block boundary at-or-before the target and no frames are
        // dropped inside it. Documented block-granular fallback.
        let payload = [0x21u8, 0x43, 0x65, 0x87];
        let mut data = Vec::new();
        for round in 0..4u16 {
            let mut block = mono_block(100 * i16::try_from(round).unwrap_or(0), 3, &payload);
            data.append(&mut block);
        }
        let file = riff(&data, &ima_fmt_hint(1, 8000, 8, 0));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full = [0f32; 40];
        let total = whole.decode_next(&file, 40, &mut full);
        assert_eq!(total, 36);

        // Mid-block target 5 lands on the block-0 boundary (frame 0).
        let mut seeked = Decoder::open(&file).expect("opens");
        assert!(seeked.seek_frames(5));
        let mut got = [0f32; 40];
        let frames = seeked.decode_next(&file, 40, &mut got);
        assert_eq!(frames, total);
        assert_eq!(&got[..frames], &full[..total]);

        // Mid-block target 12 lands on the block-1 boundary (frame 9).
        let mut seeked = Decoder::open(&file).expect("opens");
        assert!(seeked.seek_frames(12));
        let mut got = [0f32; 40];
        let frames = seeked.decode_next(&file, 40, &mut got);
        assert_eq!(frames, total - 9);
        assert_eq!(&got[..frames], &full[9..total]);
    }

    #[test]
    fn ima_seek_repositions_mid_stream_with_honest_header() {
        // The old honest-refusal case: IMA seek now repositions and the
        // resumed stream continues exactly where the sequential decode
        // would be. (Replaces the former SEEK-NOT-SUPPORTED behavior.)
        let payload = [0x70u8, 0x00, 0x00, 0x00];
        let mut data = Vec::new();
        for round in 0..3u16 {
            let mut block = mono_block(
                i16::try_from(50 * i32::from(round)).unwrap_or(0),
                0,
                &payload,
            );
            data.append(&mut block);
        }
        let file = riff(&data, &ima_fmt(1, 8000, 8));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full = [0f32; 32];
        let total = whole.decode_next(&file, 32, &mut full);
        assert_eq!(total, 27);

        let mut dec = Decoder::open(&file).expect("ima opens");
        let mut warm = [0f32; 2];
        assert_eq!(dec.decode_next(&file, 2, &mut warm), 2);
        assert!(dec.seek_frames(10));
        let mut resumed = [0f32; 32];
        let frames = dec.decode_next(&file, 32, &mut resumed);
        assert_eq!(frames, total - 10);
        assert_eq!(&resumed[..frames], &full[10..total]);
    }

    #[test]
    fn mulaw_golden_vectors_match_the_published_table() {
        // Canonical G.711 mu-law expansion (Sun g711.c form):
        // 0x00 is negative full scale, 0x80 positive full scale,
        // 0xFF/0x7F are the two zero encodings.
        assert_eq!(mulaw_to_linear(0x00), -32124);
        assert_eq!(mulaw_to_linear(0x80), 32124);
        assert_eq!(mulaw_to_linear(0xFF), 0);
        assert_eq!(mulaw_to_linear(0x7F), 0);
    }

    #[test]
    fn alaw_golden_vectors_match_the_published_table() {
        // Canonical G.711 A-law expansion: 0x00 is negative full scale
        // (-5504), 0xAA positive full scale (+32256), 0xD5/0x55 the zeros.
        assert_eq!(alaw_to_linear(0x00), -5504);
        assert_eq!(alaw_to_linear(0xAA), 32256);
        assert_eq!(alaw_to_linear(0xD5), 8);
        assert_eq!(alaw_to_linear(0x55), -8);
    }

    #[test]
    fn g711_registry_dispatch_and_normalized_decode() {
        let data = [0x00u8, 0x80, 0xFF, 0x7F];
        let file = riff(&data, &g711_fmt(7, 1, 8000));
        let mut dec = Decoder::open(&file).expect("mulaw wav opens");
        assert_eq!(dec.kind_of(), &DecoderKind::MuLaw);
        assert_eq!(dec.total_frames(), 4);
        let mut out = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut out), 4);
        assert_eq!(out[0], -32124.0 / 32768.0);
        assert_eq!(out[1], 32124.0 / 32768.0);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.0);
    }

    #[test]
    fn g711_stereo_interleaves_per_frame() {
        let data = [0x00u8, 0x80, 0xFF, 0x7F];
        let file = riff(&data, &g711_fmt(7, 2, 8000));
        let mut dec = Decoder::open(&file).expect("stereo mulaw opens");
        assert_eq!(dec.total_frames(), 2);
        let mut out = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut out), 2);
        assert_eq!(out[0], -32124.0 / 32768.0);
        assert_eq!(out[1], 32124.0 / 32768.0);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.0);
    }

    #[test]
    fn alaw_registry_dispatch_and_normalized_decode() {
        let data = [0x00u8, 0xAA];
        let file = riff(&data, &g711_fmt(6, 1, 8000));
        let mut dec = Decoder::open(&file).expect("alaw wav opens");
        assert_eq!(dec.kind_of(), &DecoderKind::ALaw);
        let mut out = [0f32; 2];
        assert_eq!(dec.decode_next(&file, 2, &mut out), 2);
        assert_eq!(out[0], -5504.0 / 32768.0);
        assert_eq!(out[1], 32256.0 / 32768.0);
    }

    #[test]
    fn g711_chunked_decode_matches_single_shot() {
        let data: Vec<u8> = (0u8..32).map(|b| b.rotate_left(1)).collect();
        let file = riff(&data, &g711_fmt(7, 1, 8000));
        let mut whole = Decoder::open(&file).expect("opens");
        let mut full_out = [0f32; 32];
        assert_eq!(whole.decode_next(&file, 32, &mut full_out), 32);

        let mut pieced = Decoder::open(&file).expect("opens");
        let mut piece_out = [0f32; 32];
        let mut filled = 0usize;
        while filled < 32 {
            let wrote = pieced.decode_next(&file, 3, &mut piece_out[filled..]);
            if wrote == 0 {
                break;
            }
            filled += wrote;
        }
        assert_eq!(filled, 32);
        assert_eq!(&full_out[..32], &piece_out[..32]);
    }

    #[test]
    fn g711_seek_repositions_byte_accurately() {
        let data: Vec<u8> = (0u8..16).map(|b| b ^ 0x55).collect();
        let file = riff(&data, &g711_fmt(7, 1, 8000));
        let mut dec = Decoder::open(&file).expect("opens");
        let mut out = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut out), 4);
        assert!(dec.seek_frames(10));
        let mut resumed = [0f32; 4];
        assert_eq!(dec.decode_next(&file, 4, &mut resumed), 4);
        let want: Vec<f32> = (10u8..14)
            .map(|b| f32::from(mulaw_to_linear(b ^ 0x55)) / 32768.0)
            .collect();
        assert_eq!(&resumed[..], &want[..]);
    }
}
