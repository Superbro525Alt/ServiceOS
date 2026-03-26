#![no_std]

use core::str;

use serviceos_abi::{ServiceId, ServiceImageId, rights};

pub const BOOT_STORE_MAGIC: [u8; 8] = *b"SOSBOOT\0";
pub const BOOT_STORE_VERSION: u32 = 1;
pub const BOOT_STORE_PATH_MAX: usize = 88;
pub const BOOT_STORE_INDEX_TEXT_MAX: usize = 2048;
pub const BOOT_STORE_MANIFEST_TEXT_MAX: usize = 2048;
pub const BOOT_STORE_MAX_DEPENDENCIES: usize = 12;
pub const BOOT_STORE_MAX_GRANTS: usize = 4;
pub const BOOT_STORE_MAX_LOOKUPS: usize = 12;
pub const BOOT_STORE_MAX_RESOURCES: usize = 4;
pub const BOOT_STORE_MAX_PACKAGE_CONTENTS: usize = 6;
pub const BOOT_STORE_MAX_PACKAGE_DEPENDENCIES: usize = 4;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootStoreEntryKind {
    Executable = 1,
    Manifest = 2,
    Data = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootStoreHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub entry_count: u32,
    pub entry_table_offset: u32,
    pub entry_size: u32,
    pub reserved: [u32; 2],
}

impl BootStoreHeader {
    pub const fn encoded_len() -> usize {
        32
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootStoreEntryRecord {
    pub kind: u32,
    pub service_id: u32,
    pub image_id: u32,
    pub flags: u32,
    pub data_offset: u32,
    pub data_len: u32,
    pub path_len: u16,
    pub reserved: u16,
    pub path: [u8; BOOT_STORE_PATH_MAX],
}

impl BootStoreEntryRecord {
    pub const fn encoded_len() -> usize {
        116
    }

    pub fn kind(&self) -> Option<BootStoreEntryKind> {
        match self.kind {
            x if x == BootStoreEntryKind::Executable as u32 => Some(BootStoreEntryKind::Executable),
            x if x == BootStoreEntryKind::Manifest as u32 => Some(BootStoreEntryKind::Manifest),
            x if x == BootStoreEntryKind::Data as u32 => Some(BootStoreEntryKind::Data),
            _ => None,
        }
    }

    pub fn path(&self) -> Result<&str, BootStoreError> {
        let len = self.path_len as usize;
        if len > self.path.len() {
            return Err(BootStoreError::InvalidPath);
        }
        str::from_utf8(&self.path[..len]).map_err(|_| BootStoreError::InvalidPath)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootStoreEntry<'a> {
    pub kind: BootStoreEntryKind,
    pub service_id: u32,
    pub image_id: u32,
    pub flags: u32,
    pub path: &'a str,
    pub data: &'a [u8],
}

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

pub struct BootStore<'a> {
    bytes: &'a [u8],
    header: BootStoreHeader,
}

impl<'a> BootStore<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, BootStoreError> {
        let header = decode_header(bytes)?;
        if header.magic != BOOT_STORE_MAGIC {
            return Err(BootStoreError::InvalidMagic);
        }
        if header.version != BOOT_STORE_VERSION {
            return Err(BootStoreError::UnsupportedVersion);
        }
        let entry_size = header.entry_size as usize;
        if entry_size != BootStoreEntryRecord::encoded_len() {
            return Err(BootStoreError::InvalidEntryTable);
        }
        let table_offset = header.entry_table_offset as usize;
        let table_len = (header.entry_count as usize)
            .checked_mul(entry_size)
            .ok_or(BootStoreError::InvalidEntryTable)?;
        let table_end = table_offset
            .checked_add(table_len)
            .ok_or(BootStoreError::InvalidEntryTable)?;
        if table_end > bytes.len() {
            return Err(BootStoreError::InvalidEntryTable);
        }

        Ok(Self { bytes, header })
    }

    pub const fn header(&self) -> &BootStoreHeader {
        &self.header
    }

    pub fn entry(&self, index: usize) -> Result<BootStoreEntry<'a>, BootStoreError> {
        if index >= self.header.entry_count as usize {
            return Err(BootStoreError::InvalidEntryTable);
        }
        let entry_size = self.header.entry_size as usize;
        let start = self.header.entry_table_offset as usize + index * entry_size;
        let end = start + entry_size;
        let record = decode_entry(&self.bytes[start..end])?;
        let kind = record.kind().ok_or(BootStoreError::InvalidEntryTable)?;
        let path_len = record.path_len as usize;
        if path_len > BOOT_STORE_PATH_MAX {
            return Err(BootStoreError::InvalidPath);
        }
        let path = str::from_utf8(&self.bytes[start + 28..start + 28 + path_len])
            .map_err(|_| BootStoreError::InvalidPath)?;
        let data_start = record.data_offset as usize;
        let data_end = data_start
            .checked_add(record.data_len as usize)
            .ok_or(BootStoreError::InvalidDataRange)?;
        if data_end > self.bytes.len() {
            return Err(BootStoreError::InvalidDataRange);
        }

        Ok(BootStoreEntry {
            kind,
            service_id: record.service_id,
            image_id: record.image_id,
            flags: record.flags,
            path,
            data: &self.bytes[data_start..data_end],
        })
    }

    pub fn resolve_image(&self, image_id: u32) -> Option<&'a [u8]> {
        for index in 0..self.header.entry_count as usize {
            let entry = self.entry(index).ok()?;
            if entry.kind == BootStoreEntryKind::Executable && entry.image_id == image_id {
                return Some(entry.data);
            }
        }
        None
    }

    pub fn find_path(&self, path: &str) -> Option<BootStoreEntry<'a>> {
        for index in 0..self.header.entry_count as usize {
            let entry = self.entry(index).ok()?;
            if entry.path == path {
                return Some(entry);
            }
        }
        None
    }
}

pub fn parse_boot_store_header(bytes: &[u8]) -> Result<BootStoreHeader, BootStoreError> {
    decode_header(bytes)
}

pub fn parse_boot_store_entry(bytes: &[u8]) -> Result<BootStoreEntryRecord, BootStoreError> {
    decode_entry(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceStartupMode {
    Eager,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartPolicy {
    OnFailure { max_restarts: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageActivationMode {
    Manual,
    Auto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceGrant {
    pub target: ServiceId,
    pub rights: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlinePath {
    len: u8,
    bytes: [u8; BOOT_STORE_PATH_MAX],
}

impl InlinePath {
    pub const fn empty() -> Self {
        Self {
            len: 0,
            bytes: [0; BOOT_STORE_PATH_MAX],
        }
    }

    pub fn set(&mut self, value: &str) -> Result<(), BootStoreError> {
        let bytes = value.as_bytes();
        if bytes.len() > self.bytes.len() {
            return Err(BootStoreError::CapacityExceeded);
        }
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len() as u8;
        Ok(())
    }

    pub fn as_str(&self) -> Result<&str, BootStoreError> {
        str::from_utf8(&self.bytes[..self.len as usize]).map_err(|_| BootStoreError::InvalidPath)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceManifest {
    pub service_id: ServiceId,
    pub image_id: ServiceImageId,
    pub startup: ServiceStartupMode,
    pub restart: RestartPolicy,
    pub dependencies: [ServiceId; BOOT_STORE_MAX_DEPENDENCIES],
    pub dependency_count: usize,
    pub grants: [ServiceGrant; BOOT_STORE_MAX_GRANTS],
    pub grant_count: usize,
    pub lookups: [ServiceGrant; BOOT_STORE_MAX_LOOKUPS],
    pub lookup_count: usize,
    pub resources: [InlinePath; BOOT_STORE_MAX_RESOURCES],
    pub resource_count: usize,
}

impl ServiceManifest {
    pub const fn empty() -> Self {
        Self {
            service_id: ServiceId::RootManager,
            image_id: ServiceImageId::RootManager,
            startup: ServiceStartupMode::Eager,
            restart: RestartPolicy::OnFailure { max_restarts: 0 },
            dependencies: [ServiceId::RootManager; BOOT_STORE_MAX_DEPENDENCIES],
            dependency_count: 0,
            grants: [ServiceGrant {
                target: ServiceId::RootManager,
                rights: rights::NONE,
            }; BOOT_STORE_MAX_GRANTS],
            grant_count: 0,
            lookups: [ServiceGrant {
                target: ServiceId::RootManager,
                rights: rights::NONE,
            }; BOOT_STORE_MAX_LOOKUPS],
            lookup_count: 0,
            resources: [InlinePath::empty(); BOOT_STORE_MAX_RESOURCES],
            resource_count: 0,
        }
    }
}

pub fn parse_manifest(bytes: &[u8]) -> Result<ServiceManifest, BootStoreError> {
    let text = str::from_utf8(bytes).map_err(|_| BootStoreError::InvalidManifest)?;
    let mut manifest = ServiceManifest::empty();
    let mut have_service = false;
    let mut have_image = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = split_key_value(line) else {
            return Err(BootStoreError::InvalidManifest);
        };

        match key {
            "service" => {
                manifest.service_id = parse_service_id(value)?;
                have_service = true;
            }
            "image" => {
                manifest.image_id = parse_image_id(value)?;
                have_image = true;
            }
            "startup" => {
                manifest.startup = match value {
                    "eager" => ServiceStartupMode::Eager,
                    _ => return Err(BootStoreError::InvalidManifest),
                };
            }
            "restart" => {
                let Some(limit) = value.strip_prefix("on-failure:") else {
                    return Err(BootStoreError::InvalidManifest);
                };
                manifest.restart = RestartPolicy::OnFailure {
                    max_restarts: parse_u32(limit)?,
                };
            }
            "depends" => {
                for entry in value.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    if manifest.dependency_count == manifest.dependencies.len() {
                        return Err(BootStoreError::CapacityExceeded);
                    }
                    manifest.dependencies[manifest.dependency_count] = parse_service_id(entry)?;
                    manifest.dependency_count += 1;
                }
            }
            "grant" => {
                if manifest.grant_count == manifest.grants.len() {
                    return Err(BootStoreError::CapacityExceeded);
                }
                manifest.grants[manifest.grant_count] = parse_service_grant(value)?;
                manifest.grant_count += 1;
            }
            "lookup" => {
                if manifest.lookup_count == manifest.lookups.len() {
                    return Err(BootStoreError::CapacityExceeded);
                }
                manifest.lookups[manifest.lookup_count] = parse_service_grant(value)?;
                manifest.lookup_count += 1;
            }
            "resource" => {
                if manifest.resource_count == manifest.resources.len() {
                    return Err(BootStoreError::CapacityExceeded);
                }
                manifest.resources[manifest.resource_count].set(value)?;
                manifest.resource_count += 1;
            }
            _ => return Err(BootStoreError::InvalidManifest),
        }
    }

    if !have_service || !have_image {
        return Err(BootStoreError::InvalidManifest);
    }

    Ok(manifest)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageManifest {
    pub package: InlinePath,
    pub version: InlinePath,
    pub compatibility: InlinePath,
    pub service_id: ServiceId,
    pub service_manifest: InlinePath,
    pub activation: PackageActivationMode,
    pub dependencies: [ServiceId; BOOT_STORE_MAX_PACKAGE_DEPENDENCIES],
    pub dependency_count: usize,
    pub contents: [InlinePath; BOOT_STORE_MAX_PACKAGE_CONTENTS],
    pub content_count: usize,
    pub integrity: u64,
}

impl PackageManifest {
    pub const fn empty() -> Self {
        Self {
            package: InlinePath::empty(),
            version: InlinePath::empty(),
            compatibility: InlinePath::empty(),
            service_id: ServiceId::RootManager,
            service_manifest: InlinePath::empty(),
            activation: PackageActivationMode::Manual,
            dependencies: [ServiceId::RootManager; BOOT_STORE_MAX_PACKAGE_DEPENDENCIES],
            dependency_count: 0,
            contents: [InlinePath::empty(); BOOT_STORE_MAX_PACKAGE_CONTENTS],
            content_count: 0,
            integrity: 0,
        }
    }
}

pub fn parse_package_manifest(bytes: &[u8]) -> Result<PackageManifest, BootStoreError> {
    let text = str::from_utf8(bytes).map_err(|_| BootStoreError::InvalidManifest)?;
    let mut manifest = PackageManifest::empty();
    let mut have_package = false;
    let mut have_version = false;
    let mut have_service = false;
    let mut have_service_manifest = false;
    let mut have_integrity = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = split_key_value(line) else {
            return Err(BootStoreError::InvalidManifest);
        };

        match key {
            "package" => {
                manifest.package.set(value)?;
                have_package = true;
            }
            "version" => {
                manifest.version.set(value)?;
                have_version = true;
            }
            "compat" => manifest.compatibility.set(value)?,
            "service" => {
                manifest.service_id = parse_service_id(value)?;
                have_service = true;
            }
            "service_manifest" => {
                manifest.service_manifest.set(value)?;
                have_service_manifest = true;
            }
            "activation" => {
                manifest.activation = match value {
                    "manual" => PackageActivationMode::Manual,
                    "auto" => PackageActivationMode::Auto,
                    _ => return Err(BootStoreError::InvalidManifest),
                };
            }
            "depends" => {
                for entry in value.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    if manifest.dependency_count == manifest.dependencies.len() {
                        return Err(BootStoreError::CapacityExceeded);
                    }
                    manifest.dependencies[manifest.dependency_count] = parse_service_id(entry)?;
                    manifest.dependency_count += 1;
                }
            }
            "content" => {
                if manifest.content_count == manifest.contents.len() {
                    return Err(BootStoreError::CapacityExceeded);
                }
                manifest.contents[manifest.content_count].set(value)?;
                manifest.content_count += 1;
            }
            "integrity" => {
                manifest.integrity = parse_integrity(value)?;
                have_integrity = true;
            }
            _ => return Err(BootStoreError::InvalidManifest),
        }
    }

    if !have_package
        || !have_version
        || !have_service
        || !have_service_manifest
        || !have_integrity
        || manifest.content_count == 0
    {
        return Err(BootStoreError::InvalidManifest);
    }
    if manifest
        .compatibility
        .as_str()
        .ok()
        .filter(|value| !value.is_empty())
        .is_none()
    {
        manifest.compatibility.set("serviceos.bootstore.v1")?;
    }

    Ok(manifest)
}

fn parse_service_grant(value: &str) -> Result<ServiceGrant, BootStoreError> {
    let Some((service, rights)) = value.split_once(':') else {
        return Err(BootStoreError::InvalidManifest);
    };
    Ok(ServiceGrant {
        target: parse_service_id(service.trim())?,
        rights: parse_rights(rights.trim())?,
    })
}

fn parse_rights(value: &str) -> Result<u64, BootStoreError> {
    let mut bits = rights::NONE;
    for part in value.split('+').map(str::trim).filter(|v| !v.is_empty()) {
        bits |= match part {
            "read" => rights::READ,
            "write" => rights::WRITE,
            "map" => rights::MAP,
            "signal" => rights::SIGNAL,
            "wait" => rights::WAIT,
            "send" => rights::SEND,
            "receive" => rights::RECEIVE,
            "duplicate" => rights::DUPLICATE,
            "transfer" => rights::TRANSFER,
            "manage" => rights::MANAGE,
            _ => return Err(BootStoreError::InvalidManifest),
        };
    }
    Ok(bits)
}

fn parse_service_id(value: &str) -> Result<ServiceId, BootStoreError> {
    match value {
        "root-manager" => Ok(ServiceId::RootManager),
        "storage-service" => Ok(ServiceId::Storage),
        "console-service" => Ok(ServiceId::Console),
        "config-service" => Ok(ServiceId::Config),
        "log-service" => Ok(ServiceId::Log),
        "status-service" => Ok(ServiceId::Status),
        "shell-service" => Ok(ServiceId::Shell),
        "package-service" => Ok(ServiceId::Package),
        "announce-service" => Ok(ServiceId::Announce),
        "network-service" => Ok(ServiceId::Network),
        "graphics-service" => Ok(ServiceId::Graphics),
        "session-service" => Ok(ServiceId::Session),
        "desktop-shell-service" => Ok(ServiceId::DesktopShell),
        _ => Err(BootStoreError::InvalidManifest),
    }
}

fn parse_image_id(value: &str) -> Result<ServiceImageId, BootStoreError> {
    match value {
        "root-manager" => Ok(ServiceImageId::RootManager),
        "storage-service" => Ok(ServiceImageId::StorageService),
        "console-service" => Ok(ServiceImageId::ConsoleService),
        "config-service" => Ok(ServiceImageId::ConfigService),
        "log-service" => Ok(ServiceImageId::LogService),
        "status-service" => Ok(ServiceImageId::StatusService),
        "shell-service" => Ok(ServiceImageId::ShellService),
        "sysinfo-tool" => Ok(ServiceImageId::SysinfoTool),
        "package-service" => Ok(ServiceImageId::PackageService),
        "announce-service" => Ok(ServiceImageId::AnnounceService),
        "network-service" => Ok(ServiceImageId::NetworkService),
        "graphics-service" => Ok(ServiceImageId::GraphicsService),
        "session-service" => Ok(ServiceImageId::SessionService),
        "desktop-shell-service" => Ok(ServiceImageId::DesktopShellService),
        _ => Err(BootStoreError::InvalidManifest),
    }
}

fn parse_u32(value: &str) -> Result<u32, BootStoreError> {
    value
        .parse::<u32>()
        .map_err(|_| BootStoreError::InvalidManifest)
}

fn parse_integrity(value: &str) -> Result<u64, BootStoreError> {
    let Some(hex) = value.strip_prefix("fnv64:0x") else {
        return Err(BootStoreError::InvalidManifest);
    };
    u64::from_str_radix(hex, 16).map_err(|_| BootStoreError::InvalidManifest)
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn decode_header(bytes: &[u8]) -> Result<BootStoreHeader, BootStoreError> {
    if bytes.len() < BootStoreHeader::encoded_len() {
        return Err(BootStoreError::Truncated);
    }
    let mut magic = [0; 8];
    magic.copy_from_slice(&bytes[..8]);
    Ok(BootStoreHeader {
        magic,
        version: read_u32(bytes, 8)?,
        entry_count: read_u32(bytes, 12)?,
        entry_table_offset: read_u32(bytes, 16)?,
        entry_size: read_u32(bytes, 20)?,
        reserved: [read_u32(bytes, 24)?, read_u32(bytes, 28)?],
    })
}

fn decode_entry(bytes: &[u8]) -> Result<BootStoreEntryRecord, BootStoreError> {
    if bytes.len() < BootStoreEntryRecord::encoded_len() {
        return Err(BootStoreError::Truncated);
    }
    let mut path = [0; BOOT_STORE_PATH_MAX];
    path.copy_from_slice(&bytes[28..28 + BOOT_STORE_PATH_MAX]);
    Ok(BootStoreEntryRecord {
        kind: read_u32(bytes, 0)?,
        service_id: read_u32(bytes, 4)?,
        image_id: read_u32(bytes, 8)?,
        flags: read_u32(bytes, 12)?,
        data_offset: read_u32(bytes, 16)?,
        data_len: read_u32(bytes, 20)?,
        path_len: read_u16(bytes, 24)?,
        reserved: read_u16(bytes, 26)?,
        path,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, BootStoreError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or(BootStoreError::Truncated)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, BootStoreError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(BootStoreError::Truncated)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parser_accepts_service_graph_schema() {
        let manifest = parse_manifest(
            br#"
service=status-service
image=status-service
startup=eager
restart=on-failure:2
depends=log-service, config-service
grant=log-service:send
lookup=config-service:send
resource=services/status-service/resources/banner.txt
"#,
        )
        .expect("manifest should parse");

        assert_eq!(manifest.service_id, ServiceId::Status);
        assert_eq!(manifest.image_id, ServiceImageId::StatusService);
        assert_eq!(manifest.dependency_count, 2);
        assert_eq!(manifest.grant_count, 1);
        assert_eq!(manifest.lookup_count, 1);
        assert_eq!(
            manifest.resources[0].as_str().expect("resource path"),
            "services/status-service/resources/banner.txt"
        );
    }

    #[test]
    fn package_manifest_parser_accepts_repository_schema() {
        let manifest = parse_package_manifest(
            br#"
package=announce-service
version=1.1.0
compat=serviceos.bootstore.v1
service=announce-service
service_manifest=packages/announce-service/1.1.0/service/manifest.svc
activation=manual
depends=log-service
content=packages/announce-service/1.1.0/service/manifest.svc
content=packages/announce-service/1.1.0/resources/message.txt
integrity=fnv64:0x1234
"#,
        )
        .expect("package manifest should parse");

        assert_eq!(manifest.service_id, ServiceId::Announce);
        assert_eq!(
            manifest
                .service_manifest
                .as_str()
                .expect("service manifest path"),
            "packages/announce-service/1.1.0/service/manifest.svc"
        );
        assert_eq!(manifest.content_count, 2);
        assert_eq!(manifest.integrity, 0x1234);
    }
}
