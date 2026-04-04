use core::str;

use serviceos_abi::{ServiceId, ServiceImageId, rights};

use crate::{
    BOOT_STORE_MAX_DEPENDENCIES, BOOT_STORE_MAX_GRANTS, BOOT_STORE_MAX_LOOKUPS,
    BOOT_STORE_MAX_PACKAGE_CONTENTS, BOOT_STORE_MAX_PACKAGE_DEPENDENCIES, BOOT_STORE_MAX_RESOURCES,
    BOOT_STORE_PATH_MAX, BootStoreError, parse_image_id, parse_integrity, parse_service_grant,
    parse_service_id, split_key_value,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceStartupMode {
    Eager,
    OnDemand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceAvailability {
    Required,
    Optional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartPolicy {
    OnFailure {
        max_restarts: u32,
        backoff_ticks: u32,
    },
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
    pub availability: ServiceAvailability,
    pub restart: RestartPolicy,
    pub ready_timeout_ticks: u32,
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
            availability: ServiceAvailability::Required,
            restart: RestartPolicy::OnFailure {
                max_restarts: 0,
                backoff_ticks: 0,
            },
            ready_timeout_ticks: 500,
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
                    "on-demand" => ServiceStartupMode::OnDemand,
                    _ => return Err(BootStoreError::InvalidManifest),
                };
            }
            "availability" => {
                manifest.availability = match value {
                    "required" => ServiceAvailability::Required,
                    "optional" => ServiceAvailability::Optional,
                    _ => return Err(BootStoreError::InvalidManifest),
                };
            }
            "restart" => {
                let Some(rest) = value.strip_prefix("on-failure:") else {
                    return Err(BootStoreError::InvalidManifest);
                };
                let mut parts = rest.split(':');
                let max_restarts =
                    crate::parse_u32(parts.next().ok_or(BootStoreError::InvalidManifest)?)?;
                let backoff_ticks = match parts.next() {
                    Some(part) => crate::parse_u32(part)?,
                    None => 0,
                };
                if parts.next().is_some() {
                    return Err(BootStoreError::InvalidManifest);
                }
                manifest.restart = RestartPolicy::OnFailure {
                    max_restarts,
                    backoff_ticks,
                };
            }
            "ready_timeout" => manifest.ready_timeout_ticks = crate::parse_u32(value)?,
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
