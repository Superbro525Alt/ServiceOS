pub(crate) const MAX_AUDIO_STREAMS: usize = 4;
pub(crate) const DEFAULT_TONE_VOLUME: u16 = u16::MAX;

/// Reply words reserved for status/frames/timestamp ahead of sample
/// payload in a `CaptureReadReply`.
pub(crate) const CAPTURE_REPLY_HEADER_WORDS: usize = 3;
/// Blocking reads yield instead of spinning; two seconds of ticks is
/// far beyond any honest capture catch-up window.
pub(crate) const CAPTURE_BLOCK_TICKS: u64 = 200;
/// Sanity clamp on a single capture read request.
pub(crate) const CAPTURE_MAX_READ_FRAMES: usize = 4096;
