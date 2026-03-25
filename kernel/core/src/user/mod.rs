use crate::memory::{
    EarlyFrameAllocator, Frame, MappingError, MappingFlags, PAGE_SIZE_BYTES, PageMapper,
    PhysicalAddress, VirtualAddress,
};

const FLAT_IMAGE_MAGIC: [u8; 8] = *b"SOSUIMG\0";
const FLAT_IMAGE_HEADER_LEN: usize = 48;
const USER_STACK_PAGES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlatImageHeader {
    pub abi_version: u32,
    pub image_base: VirtualAddress,
    pub entry_offset: u64,
    pub code_size: usize,
    pub user_stack_top: VirtualAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedUserImage {
    pub entry_point: VirtualAddress,
    pub image_base: VirtualAddress,
    pub code_size: usize,
    pub user_stack_top: VirtualAddress,
    pub mapped_stack_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    Truncated,
    InvalidMagic,
    UnsupportedAbi,
    UnsupportedHeader,
    AddressAlignment,
    FrameExhausted,
    Mapping(MappingError),
}

impl From<MappingError> for LoadError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

pub fn parse_flat_image(image: &[u8]) -> Result<FlatImageHeader, LoadError> {
    if image.len() < FLAT_IMAGE_HEADER_LEN {
        return Err(LoadError::Truncated);
    }

    if image[..FLAT_IMAGE_MAGIC.len()] != FLAT_IMAGE_MAGIC {
        return Err(LoadError::InvalidMagic);
    }

    let abi_version = read_u32_le(image, 8)?;
    let header_len = read_u32_le(image, 12)? as usize;
    if abi_version != 1 {
        return Err(LoadError::UnsupportedAbi);
    }
    if header_len != FLAT_IMAGE_HEADER_LEN {
        return Err(LoadError::UnsupportedHeader);
    }

    let image_base = VirtualAddress::new(read_u64_le(image, 16)?);
    let entry_offset = read_u64_le(image, 24)?;
    let code_size = read_u64_le(image, 32)? as usize;
    let user_stack_top = VirtualAddress::new(read_u64_le(image, 40)?);

    if image_base.as_u64() % PAGE_SIZE_BYTES != 0 || user_stack_top.as_u64() % PAGE_SIZE_BYTES != 0
    {
        return Err(LoadError::AddressAlignment);
    }
    if image.len() < header_len + code_size {
        return Err(LoadError::Truncated);
    }

    Ok(FlatImageHeader {
        abi_version,
        image_base,
        entry_offset,
        code_size,
        user_stack_top,
    })
}

pub fn load_flat_image(
    image: &[u8],
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
) -> Result<LoadedUserImage, LoadError> {
    let header = parse_flat_image(image)?;
    let code = &image[FLAT_IMAGE_HEADER_LEN..FLAT_IMAGE_HEADER_LEN + header.code_size];

    for (page_index, chunk) in code.chunks(PAGE_SIZE_BYTES as usize).enumerate() {
        let frame = allocate_zeroed_frame(frame_allocator)?;
        copy_into_frame(frame.base, chunk);
        mapper.map_page(
            header
                .image_base
                .offset((page_index as u64) * PAGE_SIZE_BYTES),
            frame,
            MappingFlags::EXECUTABLE | MappingFlags::USER_ACCESSIBLE,
            frame_allocator,
        )?;
    }

    let stack_bottom = VirtualAddress::new(
        header.user_stack_top.as_u64() - ((USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES),
    );
    for page_index in 0..USER_STACK_PAGES {
        let frame = allocate_zeroed_frame(frame_allocator)?;
        mapper.map_page(
            stack_bottom.offset((page_index as u64) * PAGE_SIZE_BYTES),
            frame,
            MappingFlags::WRITABLE | MappingFlags::USER_ACCESSIBLE,
            frame_allocator,
        )?;
    }

    Ok(LoadedUserImage {
        entry_point: header.image_base.offset(header.entry_offset),
        image_base: header.image_base,
        code_size: header.code_size,
        user_stack_top: header.user_stack_top,
        mapped_stack_bytes: USER_STACK_PAGES * PAGE_SIZE_BYTES as usize,
    })
}

fn allocate_zeroed_frame(frame_allocator: &mut EarlyFrameAllocator) -> Result<Frame, LoadError> {
    let frame = frame_allocator
        .allocate_4kib()
        .ok_or(LoadError::FrameExhausted)?;
    unsafe {
        core::ptr::write_bytes(frame.base.as_u64() as *mut u8, 0, PAGE_SIZE_BYTES as usize);
    }
    Ok(frame)
}

fn copy_into_frame(frame_base: PhysicalAddress, bytes: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), frame_base.as_u64() as *mut u8, bytes.len());
    }
}

fn read_u32_le(image: &[u8], offset: usize) -> Result<u32, LoadError> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or(LoadError::Truncated)?
        .try_into()
        .map_err(|_| LoadError::Truncated)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64_le(image: &[u8], offset: usize) -> Result<u64, LoadError> {
    let bytes = image
        .get(offset..offset + 8)
        .ok_or(LoadError::Truncated)?
        .try_into()
        .map_err(|_| LoadError::Truncated)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn build_image(
        abi_version: u32,
        image_base: u64,
        entry_offset: u64,
        code_size: u64,
        user_stack_top: u64,
        code_bytes: &[u8],
    ) -> Vec<u8> {
        let mut image = Vec::new();
        image.extend_from_slice(&FLAT_IMAGE_MAGIC);
        image.extend_from_slice(&abi_version.to_le_bytes());
        image.extend_from_slice(&(FLAT_IMAGE_HEADER_LEN as u32).to_le_bytes());
        image.extend_from_slice(&image_base.to_le_bytes());
        image.extend_from_slice(&entry_offset.to_le_bytes());
        image.extend_from_slice(&code_size.to_le_bytes());
        image.extend_from_slice(&user_stack_top.to_le_bytes());
        image.extend_from_slice(code_bytes);
        image
    }

    #[test]
    fn parse_flat_image_accepts_valid_header() {
        let image = build_image(
            1,
            0x4000_0000_0000,
            0x20,
            4,
            0x7fff_ffff_f000,
            &[1, 2, 3, 4],
        );

        let header = parse_flat_image(&image).expect("header should parse");
        assert_eq!(header.abi_version, 1);
        assert_eq!(header.image_base, VirtualAddress::new(0x4000_0000_0000));
        assert_eq!(header.entry_offset, 0x20);
        assert_eq!(header.code_size, 4);
        assert_eq!(header.user_stack_top, VirtualAddress::new(0x7fff_ffff_f000));
    }

    #[test]
    fn parse_flat_image_rejects_misaligned_addresses() {
        let image = build_image(1, 0x4000_0000_0001, 0, 4, 0x7fff_ffff_f000, &[1, 2, 3, 4]);
        assert_eq!(parse_flat_image(&image), Err(LoadError::AddressAlignment));

        let image = build_image(1, 0x4000_0000_0000, 0, 4, 0x7fff_ffff_f001, &[1, 2, 3, 4]);
        assert_eq!(parse_flat_image(&image), Err(LoadError::AddressAlignment));
    }

    #[test]
    fn parse_flat_image_rejects_wrong_abi_and_truncation() {
        let unsupported = build_image(2, 0x4000_0000_0000, 0, 4, 0x7fff_ffff_f000, &[1, 2, 3, 4]);
        assert_eq!(
            parse_flat_image(&unsupported),
            Err(LoadError::UnsupportedAbi)
        );

        let truncated = build_image(1, 0x4000_0000_0000, 0, 8, 0x7fff_ffff_f000, &[1, 2, 3, 4]);
        assert_eq!(parse_flat_image(&truncated), Err(LoadError::Truncated));
    }
}
