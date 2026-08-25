pub(crate) const MAX_ENVS: usize = 4;
pub(crate) const MAX_RUNS: usize = 4;
pub(crate) const MAX_MOUNTS: usize = 4;
pub(crate) const MAX_VARS: usize = 6;
/// Bundled guest libraries declared by an environment descriptor. Each maps
/// a guest-visible library name to a storage path holding a flat image the
/// loader can map as a dependency.
pub(crate) const MAX_LIBS: usize = 4;
/// Bytes of a staged guest image inspected before launch (ELF header plus
/// program headers, or a flat image's full fixed header).
pub(crate) const MAX_IMAGE_HEADER_BYTES: usize = 512;
pub(crate) const MAX_GUEST_PATH: usize = 48;
pub(crate) const MAX_STORAGE_PATH: usize = serviceos_bundle::BOOT_STORE_PATH_MAX;
pub(crate) const MAX_VAR_KEY: usize = 24;
pub(crate) const MAX_VAR_VALUE: usize = 64;
pub(crate) const MAX_PROFILE_BYTES: usize = 512;
pub(crate) const MAX_AUDIT: usize = 16;
