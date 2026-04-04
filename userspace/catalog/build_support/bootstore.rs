use std::error::Error;

use serviceos_bundle::{
    BOOT_STORE_MAGIC, BOOT_STORE_PATH_MAX, BOOT_STORE_VERSION, BootStoreEntryKind,
    BootStoreEntryRecord, BootStoreHeader,
};

pub(crate) struct BootStoreEntry {
    pub(crate) kind: BootStoreEntryKind,
    pub(crate) service_id: u32,
    pub(crate) image_id: u32,
    pub(crate) path: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) fn encode_bootstore(entries: &[BootStoreEntry]) -> Result<Vec<u8>, Box<dyn Error>> {
    let header_len = BootStoreHeader::encoded_len();
    let entry_len = BootStoreEntryRecord::encoded_len();
    let table_offset = header_len;
    let data_offset = table_offset + entry_len * entries.len();
    let total_len = data_offset + entries.iter().map(|entry| entry.bytes.len()).sum::<usize>();
    let mut image = vec![0u8; total_len];

    image[..8].copy_from_slice(&BOOT_STORE_MAGIC);
    image[8..12].copy_from_slice(&BOOT_STORE_VERSION.to_le_bytes());
    image[12..16].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    image[16..20].copy_from_slice(&(table_offset as u32).to_le_bytes());
    image[20..24].copy_from_slice(&(entry_len as u32).to_le_bytes());

    let mut cursor = data_offset;
    for (index, entry) in entries.iter().enumerate() {
        let entry_offset = table_offset + index * entry_len;
        let entry_end = entry_offset + entry_len;
        let record = &mut image[entry_offset..entry_end];
        record[0..4].copy_from_slice(&(entry.kind as u32).to_le_bytes());
        record[4..8].copy_from_slice(&entry.service_id.to_le_bytes());
        record[8..12].copy_from_slice(&entry.image_id.to_le_bytes());
        record[12..16].copy_from_slice(&0u32.to_le_bytes());
        record[16..20].copy_from_slice(&(cursor as u32).to_le_bytes());
        record[20..24].copy_from_slice(&(entry.bytes.len() as u32).to_le_bytes());

        let path_bytes = entry.path.as_bytes();
        if path_bytes.len() > BOOT_STORE_PATH_MAX {
            return Err(format!("boot-store path too long: {}", entry.path).into());
        }
        record[24..26].copy_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        record[28..28 + path_bytes.len()].copy_from_slice(path_bytes);
        image[cursor..cursor + entry.bytes.len()].copy_from_slice(&entry.bytes);
        cursor += entry.bytes.len();
    }

    Ok(image)
}
