#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTag {
    SnapshotRequest = 0x400,
    SnapshotReply = 0x401,
}
