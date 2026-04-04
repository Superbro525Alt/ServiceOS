#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageTag {
    OpenRequest = 0x500,
    OpenReply = 0x501,
    ReadRequest = 0x502,
    ReadReply = 0x503,
    ListRequest = 0x504,
    ListReply = 0x505,
    CloseRequest = 0x506,
    DirectoryListRequest = 0x507,
    DirectoryListReply = 0x508,
    DirectoryOpenRequest = 0x509,
    DirectoryOpenReply = 0x50a,
    DirectoryCreateRequest = 0x50b,
    DirectoryCreateReply = 0x50c,
    DirectoryRemoveRequest = 0x50d,
    DirectoryRemoveReply = 0x50e,
    DirectoryOpenFileRequest = 0x50f,
    DirectoryOpenFileReply = 0x510,
    WriteRequest = 0x511,
    WriteReply = 0x512,
    DirectoryReadRequest = 0x513,
    DirectoryReadReply = 0x514,
    MountListRequest = 0x515,
    MountListReply = 0x516,
    DirectoryTraverseRequest = 0x517,
    DirectoryTraverseReply = 0x518,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Ok = 0,
    NotFound = 1,
    InvalidPath = 2,
    InvalidOffset = 3,
    End = 4,
    Busy = 5,
    Denied = 6,
    AlreadyExists = 7,
    NotDirectory = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageEntryKind {
    File = 0,
    Directory = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageMountKind {
    Boot = 0,
    Persistent = 1,
    Ephemeral = 2,
}
