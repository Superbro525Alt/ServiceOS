#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalTag {
    SessionOpenRequest = 0xb00,
    SessionOpenReply = 0xb01,
    SessionListRequest = 0xb02,
    SessionListReply = 0xb03,
    SessionStatusRequest = 0xb04,
    SessionStatusReply = 0xb05,
    SessionInput = 0xb06,
    SessionOutput = 0xb07,
    SessionResize = 0xb08,
    SessionClose = 0xb09,
    SessionClosed = 0xb0a,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalStatus {
    Ok = 0,
    Busy = 1,
    NotFound = 2,
    Denied = 3,
    Closed = 4,
}
