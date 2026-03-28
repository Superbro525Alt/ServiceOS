use crate::{
    channel_receive_blocking, channel_send, rights, ControlTag, Error, Handle, LookupStatus,
    RawMessage, Result, ServiceId,
};

pub fn register_service(bootstrap: Handle, service_id: ServiceId, public: Handle) -> Result<()> {
    let mut register = RawMessage::empty(ControlTag::Register as u32);
    register.word_count = 1;
    register.words[0] = service_id as u32 as u64;
    register.handle_count = 1;
    register.handles[0] = public;
    register.handle_rights[0] =
        rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    channel_send(bootstrap, &register)
}

pub fn lookup_service(bootstrap: Handle, service_id: ServiceId) -> Result<Handle> {
    let mut request = RawMessage::empty(ControlTag::LookupRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut reply = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut reply)?;
    if reply.tag != ControlTag::LookupReply as u32 || reply.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match reply.words[1] as u32 {
        x if x == LookupStatus::Ok as u32 && reply.handle_count > 0 => Ok(reply.handles[0]),
        x if x == LookupStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == LookupStatus::Unavailable as u32 => Err(Error::NotFound),
        _ => Err(Error::InvalidArgument),
    }
}
