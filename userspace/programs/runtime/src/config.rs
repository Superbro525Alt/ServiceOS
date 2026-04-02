use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, rights, ConfigKey,
    ConfigStatus, ConfigTag, ConfigValueKind, Error, Handle, RawMessage, Result,
};

pub fn config_read(config_handle: Handle, key: ConfigKey) -> Result<(ConfigValueKind, u64)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConfigTag::ReadRequest as u32);
    request.word_count = 1;
    request.words[0] = key as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(config_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConfigTag::ReadReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    let kind = match response.words[1] as u32 {
        x if x == ConfigValueKind::Unsigned as u32 => ConfigValueKind::Unsigned,
        _ => return Err(Error::InvalidArgument),
    };
    Ok((kind, response.words[2]))
}

pub fn config_write(config_handle: Handle, key: ConfigKey, value: u64) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConfigTag::WriteRequest as u32);
    request.word_count = 2;
    request.words[0] = key as u32 as u64;
    request.words[1] = value;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(config_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConfigTag::WriteReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == ConfigStatus::Ok as u32 => Ok(()),
        x if x == ConfigStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == ConfigStatus::Denied as u32 => Err(Error::PermissionDenied),
        _ => Err(Error::InvalidArgument),
    }
}
