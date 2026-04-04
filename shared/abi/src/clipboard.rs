#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardTag {
    ReadRequest = 0xb20,
    ReadReply = 0xb21,
    WriteRequest = 0xb22,
    WriteReply = 0xb23,
    HistoryRequest = 0xb24,
    HistoryReply = 0xb25,
    ActivateRequest = 0xb26,
    ActivateReply = 0xb27,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardStatus {
    Ok = 0,
    NotFound = 1,
    Denied = 2,
}
