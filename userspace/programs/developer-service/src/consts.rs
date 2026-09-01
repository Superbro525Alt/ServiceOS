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

/// Farm loopback live-accept harness (SERVICEOS_FARM_SELFTEST=1 builds
/// only): one remote-target job is queued through the standard registry/
/// dispatch slot model, dispatched over guest-internal loopback TCP to an
/// in-process accept listener running the minimal FARMQ1 job-accept wire
/// protocol, and completed end-to-end (queue -> dispatch -> connect ->
/// accept -> ack -> Succeeded).
pub(crate) const FARM_SELFTEST_PORT: u16 = 44_210;
/// Local IDE-adjacent control tags, past TerminalTag-style reserved ranges
/// and next to IDE_JOB_INFO_* (shared DeveloperTag stays 0xd00-0xd0f).
pub(crate) const FARM_SELFTEST_REQUEST_TAG: u32 = 0xd22;
pub(crate) const FARM_SELFTEST_REPLY_TAG: u32 = 0xd23;
/// Per-run phase-accounting profile: request carries the job id, the reply
/// carries the five lifecycle stamps (IDE1-tail grammar, field count 6 —
/// five ticks plus the rate/valid-mask word). Reply layout is documented
/// on `reply_job_profile` in protocol.rs.
pub(crate) const DEV_PROFILE_REQUEST_TAG: u32 = 0xd24;
pub(crate) const DEV_PROFILE_REPLY_TAG: u32 = 0xd25;
