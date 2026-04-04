#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperTag {
    ToolchainListRequest = 0xd00,
    ToolchainListReply = 0xd01,
    ToolchainInfoRequest = 0xd02,
    ToolchainInfoReply = 0xd03,
    WorkspaceListRequest = 0xd04,
    WorkspaceListReply = 0xd05,
    WorkspaceInfoRequest = 0xd06,
    WorkspaceInfoReply = 0xd07,
    BuildRequest = 0xd08,
    BuildReply = 0xd09,
    JobListRequest = 0xd0a,
    JobListReply = 0xd0b,
    JobInfoRequest = 0xd0c,
    JobInfoReply = 0xd0d,
    ArtifactOpenRequest = 0xd0e,
    ArtifactOpenReply = 0xd0f,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
    Unsupported = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperTarget {
    NativeX64 = 1,
    LinuxX64 = 2,
    WindowsX64 = 3,
    MacosX64 = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperToolchainState {
    Installed = 1,
    RemoteOnly = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperArtifactFormat {
    ServiceOsFlat = 1,
    Elf64 = 2,
    Pe32Plus = 3,
    MachO64 = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperJobState {
    Queued = 1,
    Running = 2,
    Succeeded = 3,
    Failed = 4,
    Unsupported = 5,
}
