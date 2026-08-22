use core::str;

use crate::{BootStoreError, read_u16, read_u32};

pub const BOOT_STORE_MAGIC: [u8; 8] = *b"SOSBOOT\0";
pub const BOOT_STORE_VERSION: u32 = 1;
pub const BOOT_STORE_PATH_MAX: usize = 88;

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

pub struct BootStore<'a> {
    bytes: &'a [u8],
    header: BootStoreHeader,
}

impl<'a> BootStore<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, BootStoreError> {
        // Distinct early error: an oversized image can never be valid and is
        // rejected before any header field is trusted.
        if bytes.len() > crate::BOOT_STORE_MAX_BYTES {
            return Err(BootStoreError::Oversize {
                size: bytes.len(),
                max: crate::BOOT_STORE_MAX_BYTES,
            });
        }
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

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::vec;

    use super::*;

    #[test]
    fn parse_rejects_oversized_images_before_reading_header() {
        let ceiling = crate::BOOT_STORE_MAX_BYTES;
        // Build the smallest allocation that trips the guard without
        // committing 16 MiB to the test binary.
        assert!(ceiling > BootStoreHeader::encoded_len());
        let bytes = vec![0u8; ceiling + 1];
        match BootStore::parse(&bytes) {
            Err(BootStoreError::Oversize { size, max }) => {
                assert_eq!(size, ceiling + 1);
                assert_eq!(max, ceiling);
            }
            _ => panic!("oversized image must be rejected with Oversize"),
        }
    }

    #[test]
    fn parse_accepts_images_at_the_size_limit() {
        let mut bytes = vec![0u8; crate::BOOT_STORE_MAX_BYTES];
        bytes[..BOOT_STORE_MAGIC.len()].copy_from_slice(&BOOT_STORE_MAGIC);
        bytes[8..12].copy_from_slice(&BOOT_STORE_VERSION.to_le_bytes());
        // Zero entries: a valid header at exactly the ceiling parses fine.
        bytes[12..16].copy_from_slice(&0u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&(BootStoreHeader::encoded_len() as u32).to_le_bytes());
        bytes[20..24].copy_from_slice(&(BootStoreEntryRecord::encoded_len() as u32).to_le_bytes());
        assert!(BootStore::parse(&bytes).is_ok());
    }
}
