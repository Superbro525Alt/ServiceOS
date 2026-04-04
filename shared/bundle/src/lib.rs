#![no_std]

mod boot_store;
mod manifest;
mod parse;

pub const BOOT_STORE_INDEX_TEXT_MAX: usize = 2048;
pub const BOOT_STORE_MANIFEST_TEXT_MAX: usize = 2048;
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
    InvalidManifest,
}

pub use boot_store::*;
pub use manifest::*;
pub(crate) use parse::*;
