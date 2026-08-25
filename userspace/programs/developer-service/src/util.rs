use core::str;

use rt::{
    DeveloperArtifactFormat, DeveloperTarget, DeveloperToolchainState, LogDomain, LogEvent,
    LogSeverity, ServiceId, rights,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_CATALOG_BYTES, MAX_SOURCE},
    types::{FixedBytes, JobSlot, ToolchainSlot, WorkspaceSlot},
};

pub(crate) fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    detail0: u64,
    detail1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Developer,
        severity,
        LogDomain::Developer,
        event,
        detail0,
        detail1,
    )
}

pub(crate) fn read_blob_all(handle: rt::Handle, buffer: &mut [u8]) -> rt::Result<usize> {
    let mut offset = 0usize;
    while offset < buffer.len() {
        let read = rt::storage_read(handle, offset, &mut buffer[offset..])?;
        if read == 0 {
            break;
        }
        offset += read;
    }
    Ok(offset)
}

pub(crate) fn read_catalog(
    storage_handle: rt::Handle,
    catalog_handle: rt::Handle,
    toolchains: &mut [ToolchainSlot],
    workspaces: &mut [WorkspaceSlot],
) -> rt::Result<(usize, usize)> {
    let mut catalog_bytes = [0u8; MAX_CATALOG_BYTES];
    let loaded = read_blob_all(catalog_handle, &mut catalog_bytes)?;
    let catalog =
        str::from_utf8(&catalog_bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    let mut descriptor_bytes = [0u8; MAX_CATALOG_BYTES];

    let mut toolchain_count = 0usize;
    let mut workspace_count = 0usize;

    for line in catalog
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(path) = line.strip_prefix("toolchain=") {
            let descriptor = read_storage_text(storage_handle, path.trim(), &mut descriptor_bytes)?;
            if toolchain_count >= toolchains.len() {
                return Err(rt::Error::CapacityExceeded);
            }
            toolchains[toolchain_count] = parse_toolchain_descriptor(descriptor)?;
            toolchains[toolchain_count].occupied = true;
            toolchain_count += 1;
        } else if let Some(path) = line.strip_prefix("workspace=") {
            let descriptor = read_storage_text(storage_handle, path.trim(), &mut descriptor_bytes)?;
            if workspace_count >= workspaces.len() {
                return Err(rt::Error::CapacityExceeded);
            }
            workspaces[workspace_count] = parse_workspace_descriptor(descriptor, toolchains)?;
            workspaces[workspace_count].occupied = true;
            workspace_count += 1;
        }
    }

    Ok((toolchain_count, workspace_count))
}

fn read_storage_text<'a>(
    storage_handle: rt::Handle,
    path: &str,
    buffer: &'a mut [u8],
) -> rt::Result<&'a str> {
    let (blob, _) = rt::storage_open(storage_handle, path)?;
    let loaded = read_blob_all(blob, buffer)?;
    let _ = rt::storage_blob_close(blob);
    str::from_utf8(&buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)
}

fn parse_toolchain_descriptor(text: &str) -> rt::Result<ToolchainSlot> {
    let mut slot = ToolchainSlot::empty();
    // `checksum=` applies to the most recent `payload=` line.
    let mut last_payload: Option<usize> = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "name" => slot.name.set(value.as_bytes())?,
            "target" => slot.target = parse_target(value)?,
            "state" => slot.state = parse_toolchain_state(value)?,
            "format" => slot.format = parse_format(value)?,
            "sdk_root" => slot.sdk_root.set(value.as_bytes())?,
            "remote_endpoint" => slot.remote_endpoint.set(value.as_bytes())?,
            "payload" => {
                if slot.payload_count >= slot.payloads.len() {
                    return Err(rt::Error::CapacityExceeded);
                }
                slot.payloads[slot.payload_count] =
                    crate::payload::parse_payload_entry(value)?;
                last_payload = Some(slot.payload_count);
                slot.payload_count += 1;
            }
            "checksum" => {
                let Some(index) = last_payload else {
                    return Err(rt::Error::InvalidArgument);
                };
                let raw = crate::payload::parse_checksum(value)?;
                let mut hex = FixedBytes::<64>::empty();
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let mut encoded = [0u8; 64];
                for (index, byte) in raw.as_bytes().iter().enumerate() {
                    encoded[index * 2] = HEX[(byte >> 4) as usize];
                    encoded[index * 2 + 1] = HEX[(byte & 0xF) as usize];
                }
                hex.set(&encoded)?;
                slot.payloads[index].checksum_hex = hex;
            }
            _ => {}
        }
    }
    Ok(slot)
}

fn parse_workspace_descriptor(
    text: &str,
    toolchains: &[ToolchainSlot],
) -> rt::Result<WorkspaceSlot> {
    let mut slot = WorkspaceSlot::empty();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "name" => slot.name.set(value.as_bytes())?,
            "artifact" => slot.artifact.set(value.as_bytes())?,
            "source" => slot.source_path.set(value.as_bytes())?,
            "native_toolchain" => slot.toolchains[0] = find_toolchain(toolchains, value)?,
            "linux_toolchain" => slot.toolchains[1] = find_toolchain(toolchains, value)?,
            "windows_toolchain" => slot.toolchains[2] = find_toolchain(toolchains, value)?,
            "macos_toolchain" => slot.toolchains[3] = find_toolchain(toolchains, value)?,
            _ => {}
        }
    }
    Ok(slot)
}

fn find_toolchain(toolchains: &[ToolchainSlot], name: &str) -> rt::Result<u32> {
    for (index, toolchain) in toolchains.iter().enumerate() {
        if toolchain.occupied && toolchain.name.as_bytes() == name.as_bytes() {
            return Ok(index as u32);
        }
    }
    Err(rt::Error::NotFound)
}

fn parse_target(value: &str) -> rt::Result<DeveloperTarget> {
    match value {
        "native-x64" => Ok(DeveloperTarget::NativeX64),
        "linux-x64" => Ok(DeveloperTarget::LinuxX64),
        "windows-x64" => Ok(DeveloperTarget::WindowsX64),
        "macos-x64" => Ok(DeveloperTarget::MacosX64),
        _ => Err(rt::Error::InvalidArgument),
    }
}

fn parse_toolchain_state(value: &str) -> rt::Result<DeveloperToolchainState> {
    match value {
        "installed" => Ok(DeveloperToolchainState::Installed),
        "remote-only" => Ok(DeveloperToolchainState::RemoteOnly),
        _ => Err(rt::Error::InvalidArgument),
    }
}

fn parse_format(value: &str) -> rt::Result<DeveloperArtifactFormat> {
    match value {
        "serviceos-flat" => Ok(DeveloperArtifactFormat::ServiceOsFlat),
        "elf64" => Ok(DeveloperArtifactFormat::Elf64),
        "pe32+" => Ok(DeveloperArtifactFormat::Pe32Plus),
        "macho64" => Ok(DeveloperArtifactFormat::MachO64),
        _ => Err(rt::Error::InvalidArgument),
    }
}

pub(crate) fn target_slot_index(target: DeveloperTarget) -> usize {
    match target {
        DeveloperTarget::NativeX64 => 0,
        DeveloperTarget::LinuxX64 => 1,
        DeveloperTarget::WindowsX64 => 2,
        DeveloperTarget::MacosX64 => 3,
    }
}

pub(crate) fn workspace_target_mask(workspace: &WorkspaceSlot) -> u32 {
    let mut mask = 0u32;
    for (index, toolchain) in workspace.toolchains.iter().copied().enumerate() {
        if toolchain != u32::MAX {
            mask |= 1u32 << index;
        }
    }
    mask
}

pub(crate) fn allocate_job(jobs: &mut [JobSlot]) -> rt::Result<usize> {
    jobs.iter()
        .position(|job| !job.occupied)
        .ok_or(rt::Error::CapacityExceeded)
}

pub(crate) fn release_job(job: &mut JobSlot) {
    if job.report_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(job.report_handle);
    }
    if job.task_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(job.task_handle);
    }
    if job.artifact_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(job.artifact_handle);
    }
    *job = JobSlot::empty();
}

pub(crate) fn load_source_into_memory(
    storage_handle: rt::Handle,
    workspace: &WorkspaceSlot,
) -> rt::Result<(rt::Handle, usize)> {
    let path = core::str::from_utf8(workspace.source_path.as_bytes())
        .map_err(|_| rt::Error::InvalidArgument)?;
    let (blob, _) = rt::storage_open(storage_handle, path)?;
    let mut source = [0u8; MAX_SOURCE];
    let loaded = read_blob_all(blob, &mut source)?;
    let _ = rt::storage_blob_close(blob);
    let memory = rt::memory_create(loaded, true)?;
    let _ = rt::memory_write(memory, 0, &source[..loaded])?;
    Ok((memory, loaded))
}

pub(crate) fn send_builder_log(output_handle: rt::Handle, text: &str) {
    let _ = rt::text_relay_write(output_handle, text);
}

pub(crate) fn create_memory_from_bytes(bytes: &[u8]) -> rt::Result<rt::Handle> {
    let memory = rt::memory_create(bytes.len(), true)?;
    match rt::memory_write(memory, 0, bytes) {
        Ok(_) => Ok(memory),
        Err(error) => {
            let _ = rt::handle_close(memory);
            Err(error)
        }
    }
}

pub(crate) fn duplicate_artifact_for_reply(handle: rt::Handle) -> rt::Result<rt::Handle> {
    rt::handle_duplicate(handle, rights::READ | rights::DUPLICATE | rights::TRANSFER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_descriptor_parses_remote_endpoint() {
        let text = "name=macos-x64\r\ntarget=macos-x64\r\nstate=remote-only\r\nformat=macho64\r\nsdk_root=sdk/macos\r\nremote_endpoint=farm@10.0.0.9:7900\r\n";
        let slot = parse_toolchain_descriptor(text).unwrap();
        assert_eq!(slot.remote_endpoint.as_bytes(), b"farm@10.0.0.9:7900");
        assert_eq!(slot.state, DeveloperToolchainState::RemoteOnly);
    }

    #[test]
    fn toolchain_descriptor_defaults_to_unconfigured() {
        let slot = parse_toolchain_descriptor("name=macos-x64\nstate=remote-only\n").unwrap();
        assert!(!slot.configured());
    }

    #[test]
    fn toolchain_descriptor_parses_payloads_and_checksum() {
        let text = "name=linux-x64\ntarget=linux-x64\nstate=installed\nformat=elf64\n\
                    sdk_root=sdk/linux\n\
                    payload=metadata@path:packages/d/sdk/linux/README.txt\n\
                    checksum=BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD\n\
                    payload=builder@image:24\n";
        let slot = parse_toolchain_descriptor(text).unwrap();
        assert_eq!(slot.payload_count, 2);
        assert_eq!(
            slot.payloads[0].reference,
            crate::payload::PayloadRef::StoragePath({
                let mut path = crate::types::FixedBytes::<96>::empty();
                path.set(b"packages/d/sdk/linux/README.txt").unwrap();
                path
            })
        );
        assert!(slot.payloads[0].wants_checksum());
        // Uppercase descriptor hex is normalized through the raw bytes.
        assert_eq!(
            slot.payloads[0].checksum_hex.as_bytes()[..8],
            *b"ba7816bf"
        );
        assert!(!slot.payloads[1].wants_checksum());
    }

    #[test]
    fn toolchain_descriptor_rejects_checksum_without_payload() {
        let text = "name=x\nchecksum=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n";
        assert!(parse_toolchain_descriptor(text).is_err());
    }

    #[test]
    fn toolchain_descriptor_rejects_bad_payload_refs() {
        assert!(parse_toolchain_descriptor("name=x\npayload=noat\n").is_err());
        assert!(parse_toolchain_descriptor(
            "name=x\npayload=a@image:notanumber\n"
        )
        .is_err());
    }
}
