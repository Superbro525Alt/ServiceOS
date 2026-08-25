pub(crate) const MAX_TOOLCHAINS: usize = 8;
pub(crate) const MAX_WORKSPACES: usize = 8;
pub(crate) const MAX_JOBS: usize = 8;
pub(crate) const MAX_RUNTIMES: usize = 4;
pub(crate) const MAX_NAME: usize = 64;
pub(crate) const MAX_PATH: usize = 96;
pub(crate) const MAX_SOURCE: usize = 256;
pub(crate) const MAX_CATALOG_BYTES: usize = 512;
pub(crate) const BUILDER_REPORT_TAG: u32 = 1;
/// Local IDE/editor query tags, outside the shared DeveloperTag range
/// (0xd00-0xd0f): machine-readable job snapshot request and reply. Reply
/// layout is documented on `reply_ide_job_info` in protocol.rs.
pub(crate) const IDE_JOB_INFO_REQUEST_TAG: u32 = 0xd20;
pub(crate) const IDE_JOB_INFO_REPLY_TAG: u32 = 0xd21;
