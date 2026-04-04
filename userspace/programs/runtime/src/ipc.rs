use crate::{
    syscall1, syscall2, Error, Handle, HandlePair, RawMessage, Result, SyscallNumber,
    IPC_FLAG_NONBLOCK, INVALID_HANDLE, rights,
};

pub fn channel_create() -> Result<HandlePair> {
    let mut pair = HandlePair {
        first: INVALID_HANDLE,
        second: INVALID_HANDLE,
    };
    syscall1(SyscallNumber::ChannelCreate, &mut pair as *mut HandlePair as u64)?;
    Ok(pair)
}

pub fn channel_send(endpoint: Handle, message: &RawMessage) -> Result<()> {
    syscall2(
        SyscallNumber::ChannelSend,
        endpoint as u64,
        message as *const RawMessage as u64,
    )
    .map(|_| ())
}

pub fn channel_send_blocking(endpoint: Handle, message: &RawMessage) -> Result<()> {
    loop {
        match channel_send(endpoint, message) {
            Ok(()) => return Ok(()),
            Err(Error::CapacityExceeded) => {
                crate::yield_current()?;
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn channel_receive(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    syscall2(
        SyscallNumber::ChannelReceive,
        endpoint as u64,
        message as *mut RawMessage as u64,
    )
    .map(|_| ())
}

pub fn channel_receive_nonblocking(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    message.flags = IPC_FLAG_NONBLOCK;
    let result = channel_receive(endpoint, message);
    message.flags = 0;
    result
}

pub fn channel_receive_blocking(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    loop {
        match channel_receive(endpoint, message) {
            Ok(()) => return Ok(()),
            Err(Error::QueueEmpty) => {}
            Err(error) => return Err(error),
        }
    }
}

pub fn channel_call(endpoint: Handle, request: &mut RawMessage) -> Result<RawMessage> {
    let reply = channel_create()?;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;

    let send_result = channel_send_blocking(endpoint, request);
    let _ = crate::handle_close(reply.second);
    send_result?;

    let mut response = RawMessage::empty(0);
    let receive_result = channel_receive_blocking(reply.first, &mut response);
    let _ = crate::handle_close(reply.first);
    receive_result?;
    Ok(response)
}
