use serviceos_bundle::{parse_manifest, BOOT_STORE_PATH_MAX};
use serviceos_userspace_runtime as rt;

use crate::{
    state::{ServiceSlot, MAX_MANIFEST_BYTES, MAX_SERVICE_SLOTS},
    util::find_slot_index,
};

pub(crate) fn load_manifest_from_storage(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    path: &str,
) -> rt::Result<serviceos_bundle::ServiceManifest> {
    let storage_index = find_slot_index(slots, service_count, rt::ServiceId::Storage)?;
    let storage_handle = slots[storage_index].public_handle;
    let (manifest_handle, manifest_len) = rt::storage_open(storage_handle, path)?;
    let mut manifest_buffer = [0u8; MAX_MANIFEST_BYTES];
    let requested = manifest_len.min(manifest_buffer.len());
    let loaded = rt::storage_read_all(manifest_handle, &mut manifest_buffer, requested)?;
    let _ = rt::storage_blob_close(manifest_handle);
    parse_manifest(&manifest_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)
}

pub(super) fn load_image_from_storage(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    path: &str,
) -> rt::Result<rt::Handle> {
    let storage_index = find_slot_index(slots, service_count, rt::ServiceId::Storage)?;
    let storage_handle = slots[storage_index].public_handle;
    let (blob_handle, blob_len) = rt::storage_open(storage_handle, path)?;
    let image_handle = rt::memory_create(blob_len, true)?;
    let mut offset = 0usize;
    let mut chunk = [0u8; 96];
    while offset < blob_len {
        let read = rt::storage_read(blob_handle, offset, &mut chunk)?;
        if read == 0 {
            break;
        }
        let _ = rt::memory_write(image_handle, offset, &chunk[..read])?;
        offset += read;
    }
    let _ = rt::storage_blob_close(blob_handle);
    Ok(image_handle)
}

pub(super) fn unpack_path<'a>(
    words: &[u64],
    path_len: usize,
    path_bytes: &'a mut [u8; BOOT_STORE_PATH_MAX],
) -> rt::Result<&'a str> {
    crate::util::unpack_bytes(words, path_len, path_bytes).map_err(|_| rt::Error::InvalidArgument)?;
    core::str::from_utf8(&path_bytes[..path_len]).map_err(|_| rt::Error::InvalidArgument)
}
