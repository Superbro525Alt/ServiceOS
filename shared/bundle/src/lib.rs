#![no_std]

mod boot_store;
mod manifest;
mod parse;

pub const BOOT_STORE_INDEX_TEXT_MAX: usize = 2048;
pub const BOOT_STORE_MANIFEST_TEXT_MAX: usize = 2048;
/// Hard upper bound on an entire boot-store image. The store is embedded into
/// the kernel image, so unbounded growth silently bloats kernel8.img; both
/// the catalog build and the parser enforce this ceiling.
pub const BOOT_STORE_MAX_BYTES: usize = 16 * 1024 * 1024;
pub const BOOT_STORE_MAX_DEPENDENCIES: usize = 12;
pub const BOOT_STORE_MAX_GRANTS: usize = 4;
pub const BOOT_STORE_MAX_LOOKUPS: usize = 16;
pub const BOOT_STORE_MAX_RESOURCES: usize = 4;
pub const BOOT_STORE_MAX_PACKAGE_CONTENTS: usize = 16;
pub const BOOT_STORE_MAX_PACKAGE_DEPENDENCIES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootStoreError {
    Truncated,
    InvalidMagic,
    UnsupportedVersion,
    InvalidEntryTable,
    InvalidPath,
    InvalidDataRange,
    CapacityExceeded,
    /// The whole store image exceeds [`BOOT_STORE_MAX_BYTES`].
    Oversize {
        size: usize,
        max: usize,
    },
    InvalidManifest,
}

pub use boot_store::*;
pub use manifest::*;
pub(crate) use parse::*;
