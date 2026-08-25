use serviceos_userspace_runtime::{
    AudioSampleFormat, AudioStatus, AudioStreamDirection, AudioTag, Error, Handle, RawMessage,
    Result, channel_call, pcm_pack_words,
};

fn status_error(status: AudioStatus) -> Error {
    match status {
        AudioStatus::Ok => Error::Unsupported,
        AudioStatus::NotFound => Error::NotFound,
        AudioStatus::Busy => Error::Busy,
        AudioStatus::Unsupported => Error::Unsupported,
        AudioStatus::Denied => Error::PermissionDenied,
        AudioStatus::CapacityExceeded => Error::CapacityExceeded,
        AudioStatus::Closed => Error::InvalidCall,
    }
}

fn status_of(word: u64) -> Option<AudioStatus> {
    Some(match word as u32 {
        x if x == AudioStatus::Ok as u32 => AudioStatus::Ok,
        x if x == AudioStatus::NotFound as u32 => AudioStatus::NotFound,
        x if x == AudioStatus::Busy as u32 => AudioStatus::Busy,
        x if x == AudioStatus::Unsupported as u32 => AudioStatus::Unsupported,
        x if x == AudioStatus::Denied as u32 => AudioStatus::Denied,
        x if x == AudioStatus::CapacityExceeded as u32 => AudioStatus::CapacityExceeded,
        x if x == AudioStatus::Closed as u32 => AudioStatus::Closed,
        _ => return None,
    })
}

fn format_of(word: u64) -> Option<AudioSampleFormat> {
    Some(match word as u32 {
        x if x == AudioSampleFormat::U8 as u32 => AudioSampleFormat::U8,
        x if x == AudioSampleFormat::S16Le as u32 => AudioSampleFormat::S16Le,
        x if x == AudioSampleFormat::S32Le as u32 => AudioSampleFormat::S32Le,
        x if x == AudioSampleFormat::F32Le as u32 => AudioSampleFormat::F32Le,
        _ => return None,
    })
}

fn ok_status(reply: &RawMessage, reply_tag: AudioTag, min_words: usize) -> Result<()> {
    if reply.tag != reply_tag as u32 || reply.word_count < min_words as u32 {
        return Err(Error::InvalidArgument);
    }
    let status = status_of(reply.words[0]).ok_or(Error::InvalidArgument)?;
    if status != AudioStatus::Ok {
        return Err(status_error(status));
    }
    Ok(())
}

/// Opens a playback PCM stream; returns the stream handle.
pub(crate) fn stream_open(audio_handle: Handle) -> Result<Handle> {
    let mut request = RawMessage::empty(AudioTag::StreamOpenRequest as u32);
    request.word_count = 2;
    request.words[0] = AudioStreamDirection::Playback as u32 as u64;
    request.words[1] = u64::from(crate::state::SESSION_ID);
    let response = channel_call(audio_handle, &mut request)?;
    if response.tag != AudioTag::StreamOpenReply as u32
        || response.word_count < 1
        || response.handle_count < 1
    {
        return Err(Error::InvalidArgument);
    }
    let status = status_of(response.words[0]).ok_or(Error::InvalidArgument)?;
    if status != AudioStatus::Ok {
        return Err(status_error(status));
    }
    Ok(response.handles[0])
}

/// Negotiates the stream format; returns the accepted (format, rate,
/// channels) and the sink rate for honest display.
pub(crate) fn stream_configure(
    stream_handle: Handle,
    format: AudioSampleFormat,
    rate: u32,
    channels: u32,
) -> Result<(AudioSampleFormat, u32, u32, u32)> {
    let mut request = RawMessage::empty(AudioTag::StreamConfigureRequest as u32);
    request.word_count = 3;
    request.words[0] = format as u32 as u64;
    request.words[1] = rate as u64;
    request.words[2] = channels as u64;
    let response = channel_call(stream_handle, &mut request)?;
    ok_status(&response, AudioTag::StreamConfigureReply, 5)?;
    let accepted_format = format_of(response.words[1]).ok_or(Error::InvalidArgument)?;
    Ok((
        accepted_format,
        response.words[2] as u32,
        response.words[3] as u32,
        response.words[4] as u32,
    ))
}

/// Nonblocking PCM write of one packed chunk. Returns queued frames.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_write(
    stream_handle: Handle,
    frame_count: usize,
    words: &[u64],
) -> Result<usize> {
    let mut request = RawMessage::empty(AudioTag::StreamWriteRequest as u32);
    request.word_count = (2 + words.len()) as u32;
    request.words[0] = frame_count as u64;
    request.words[1] = 0;
    request.words[2..2 + words.len()].copy_from_slice(words);
    let response = channel_call(stream_handle, &mut request)?;
    if response.tag != AudioTag::StreamWriteReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    match status_of(response.words[0]) {
        Some(AudioStatus::Ok) => Ok(response.words[1] as usize),
        Some(AudioStatus::Busy) => Err(Error::Busy),
        Some(status) => Err(status_error(status)),
        None => Err(Error::InvalidArgument),
    }
}

/// Per-stream volume in percent with a mute flag.
pub(crate) fn stream_set_volume(
    stream_handle: Handle,
    volume_percent: u8,
    muted: bool,
) -> Result<(u8, bool)> {
    let mut request = RawMessage::empty(AudioTag::StreamSetVolumeRequest as u32);
    request.word_count = 2;
    request.words[0] = u64::from(volume_percent.min(100));
    request.words[1] = u64::from(muted);
    let response = channel_call(stream_handle, &mut request)?;
    ok_status(&response, AudioTag::StreamSetVolumeReply, 3)?;
    Ok((response.words[1].min(100) as u8, response.words[2] != 0))
}

/// Blocks until the sink has drained every queued frame.
pub(crate) fn stream_drain(stream_handle: Handle) -> Result<u64> {
    let mut request = RawMessage::empty(AudioTag::StreamDrainRequest as u32);
    let response = channel_call(stream_handle, &mut request)?;
    ok_status(&response, AudioTag::StreamDrainReply, 2)?;
    Ok(response.words[1])
}

pub(crate) fn stream_close(stream_handle: Handle) -> Result<()> {
    let mut request = RawMessage::empty(AudioTag::StreamCloseRequest as u32);
    let response = channel_call(stream_handle, &mut request)?;
    if response.tag != AudioTag::StreamCloseReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match status_of(response.words[0]) {
        Some(AudioStatus::Ok) | Some(AudioStatus::Closed) => Ok(()),
        Some(status) => Err(status_error(status)),
        None => Err(Error::InvalidArgument),
    }
}

/// Converts interleaved f32 samples into packed IPC words.
pub(crate) fn pack_samples(format: AudioSampleFormat, samples: &[f32], out: &mut [u64]) -> usize {
    pcm_pack_words(format, samples, out)
}
