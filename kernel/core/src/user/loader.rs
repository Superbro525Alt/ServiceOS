use crate::memory::{
    EarlyFrameAllocator, Frame, MappingFlags, PAGE_SIZE_BYTES, PageMapper, PhysicalAddress,
    USER_SPACE_END, VirtualAddress,
};

use super::{
    FlatDependencyRecord, FlatImageHeader, FlatSegmentRecord, KERNEL_ABI_VERSION, LoadError,
    LoadedLibraryRecord, LoadedUserImage, MAX_FLAT_DEPENDENCIES, MAX_FLAT_SEGMENTS,
    flat_image_policy,
    types::{FLAT_IMAGE_HEADER_LEN, FLAT_IMAGE_HEADER_LEN_V2, USER_STACK_PAGES, flat_image_magic},
};

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LSB: u8 = 1;
const ELF_VERSION_CURRENT: u8 = 1;
const ELF_TYPE_EXEC: u16 = 2;
const ELF_MACHINE_X86_64: u16 = 62;
const ELF_MACHINE_AARCH64: u16 = 183;
const ELF_PROGRAM_HEADER_LEN: usize = 56;
const ELF_SEGMENT_LOAD: u32 = 1;
/// `PT_GNU_STACK`: presence records the stack execute policy for the image.
const ELF_SEGMENT_GNU_STACK: u32 = 0x6474_e551;
const ELF_FLAG_EXECUTE: u32 = 1;
const ELF_FLAG_WRITE: u32 = 2;

/// Every userspace image must map entirely inside this window. The image
/// builder places images at the window start with stacks just below
/// [`USER_SPACE_END`]; anything outside cannot belong to a user address space
/// and would alias kernel mappings (the higher-half heap or page-table frames
/// reachable through the identity map) inside shared parent tables.
const USER_IMAGE_WINDOW_START: u64 = 0x0000_4000_0000_0000;

fn image_window_contains(start: u64, length: u64) -> bool {
    start >= USER_IMAGE_WINDOW_START
        && start < USER_SPACE_END.as_u64()
        && start
            .checked_add(length)
            .is_some_and(|end| end <= USER_SPACE_END.as_u64())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfMachine {
    X86_64,
    Aarch64,
}

impl ElfMachine {
    fn as_u16(self) -> u16 {
        match self {
            Self::X86_64 => ELF_MACHINE_X86_64,
            Self::Aarch64 => ELF_MACHINE_AARCH64,
        }
    }
}

pub fn parse_flat_image(image: &[u8]) -> Result<FlatImageHeader, LoadError> {
    if image.len() < FLAT_IMAGE_HEADER_LEN {
        return Err(LoadError::Truncated);
    }

    if image[..flat_image_magic().len()] != flat_image_magic() {
        return Err(LoadError::InvalidMagic);
    }

    let abi_version = read_u32_le(image, 8)?;
    let header_len = read_u32_le(image, 12)? as usize;
    if abi_version != 1 {
        return Err(LoadError::UnsupportedAbi);
    }
    if header_len != FLAT_IMAGE_HEADER_LEN && header_len != FLAT_IMAGE_HEADER_LEN_V2 {
        return Err(LoadError::UnsupportedHeader);
    }

    let image_base = VirtualAddress::new(read_u64_le(image, 16)?);
    let entry_offset = read_u64_le(image, 24)?;
    let file_size = read_u64_le(image, 32)? as usize;
    let executable_limit = read_u64_le(image, 40)? as usize;
    let writable_offset = read_u64_le(image, 48)? as usize;
    let memory_size = read_u64_le(image, 56)? as usize;
    let user_stack_top = VirtualAddress::new(read_u64_le(image, 64)?);

    if image_base.as_u64() % PAGE_SIZE_BYTES != 0 || user_stack_top.as_u64() % PAGE_SIZE_BYTES != 0
    {
        return Err(LoadError::AddressAlignment);
    }
    if !image_window_contains(image_base.as_u64(), memory_size as u64)
        || !image_window_contains(
            user_stack_top
                .as_u64()
                .saturating_sub((USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES),
            (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES,
        )
    {
        return Err(LoadError::UnsupportedHeader);
    }
    if image.len() < header_len + file_size {
        return Err(LoadError::Truncated);
    }
    if file_size == 0
        || executable_limit == 0
        || executable_limit > file_size
        || writable_offset > memory_size
        || file_size > memory_size
    {
        return Err(LoadError::Truncated);
    }

    // Legacy headers keep the zero-valued extension fields.
    let mut format_flags = 0u32;
    let mut min_kernel_abi = 0u32;
    let mut dependencies = [FlatDependencyRecord {
        image_id: 0,
        base_offset_hint: 0,
    }; MAX_FLAT_DEPENDENCIES];
    let mut dependency_count = 0usize;
    let mut segments = [FlatSegmentRecord {
        virtual_offset: 0,
        file_offset: 0,
        file_size: 0,
        memory_size: 0,
        executable: false,
        writable: false,
    }; MAX_FLAT_SEGMENTS];
    let mut segment_count = 0usize;

    if header_len == FLAT_IMAGE_HEADER_LEN_V2 {
        format_flags = read_u32_le(image, 72)?;
        min_kernel_abi = read_u32_le(image, 76)?;
        dependency_count = read_u32_le(image, 80)? as usize;
        segment_count = read_u32_le(image, 84)? as usize;
        if format_flags & !flat_image_policy::VALID_MASK != 0 {
            return Err(LoadError::UnsupportedHeader);
        }
        if min_kernel_abi > KERNEL_ABI_VERSION {
            return Err(LoadError::KernelAbiTooNew);
        }
        if dependency_count > MAX_FLAT_DEPENDENCIES || segment_count > MAX_FLAT_SEGMENTS {
            return Err(LoadError::UnsupportedHeader);
        }
        if uses_segment_table_flag(format_flags) && segment_count == 0 {
            return Err(LoadError::UnsupportedHeader);
        }
        for index in 0..dependency_count {
            let base = 88 + index * 16;
            dependencies[index] = FlatDependencyRecord {
                image_id: read_u32_le(image, base)?,
                base_offset_hint: read_u64_le(image, base + 8)?,
            };
        }
        for index in 0..segment_count {
            let base = 152 + index * 32;
            let flags = read_u32_le(image, base + 28)?;
            segments[index] = FlatSegmentRecord {
                virtual_offset: read_u64_le(image, base)?,
                file_offset: read_u64_le(image, base + 8)?,
                file_size: read_u64_le(image, base + 16)?,
                memory_size: read_u32_le(image, base + 24)? as u64,
                executable: flags & 1 != 0,
                writable: flags & 2 != 0,
            };
        }
        if uses_segment_table_flag(format_flags) {
            validate_segment_table(segment_count, &segments, memory_size, file_size)?;
        }
    }

    Ok(FlatImageHeader {
        abi_version,
        header_len,
        image_base,
        entry_offset,
        file_size,
        executable_limit,
        writable_offset,
        memory_size,
        user_stack_top,
        format_flags,
        min_kernel_abi,
        dependencies,
        dependency_count,
        segments,
        segment_count,
    })
}

fn uses_segment_table_flag(format_flags: u32) -> bool {
    format_flags & flat_image_policy::SEGMENT_TABLE != 0
}

/// Segment-table images must describe disjoint page-aligned regions that fit
/// inside the declared image footprint and payload.
fn validate_segment_table(
    segment_count: usize,
    segments: &[FlatSegmentRecord; MAX_FLAT_SEGMENTS],
    memory_size: usize,
    file_size: usize,
) -> Result<(), LoadError> {
    let page_size = PAGE_SIZE_BYTES as u64;
    for segment in &segments[..segment_count] {
        if segment.virtual_offset % page_size != 0
            || segment.memory_size == 0
            || segment.file_size > segment.memory_size
            || segment.virtual_offset + segment.memory_size > memory_size as u64
            || segment.file_offset + segment.file_size > file_size as u64
        {
            return Err(LoadError::UnsupportedHeader);
        }
    }
    for left in 0..segment_count {
        for right in (left + 1)..segment_count {
            let a = segments[left];
            let b = segments[right];
            let a_page_start = a.virtual_offset;
            let a_page_end = align_up_u64(a.virtual_offset + a.memory_size, page_size);
            let b_page_start = b.virtual_offset;
            let b_page_end = align_up_u64(b.virtual_offset + b.memory_size, page_size);
            if a_page_start < b_page_end && b_page_start < a_page_end {
                return Err(LoadError::UnsupportedHeader);
            }
        }
    }
    Ok(())
}

fn align_up_u64(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

pub fn load_flat_image(
    image: &[u8],
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
) -> Result<LoadedUserImage, LoadError> {
    let header = parse_flat_image(image)?;
    let payload = &image[header.header_len..header.header_len + header.file_size];

    let page_size = PAGE_SIZE_BYTES as usize;
    let image_pages = header.memory_size.div_ceil(page_size);
    for page_index in 0..image_pages {
        let page_offset = page_index * page_size;
        let page_end = page_offset.saturating_add(page_size);
        let frame = allocate_zeroed_frame(frame_allocator)?;
        if page_offset < payload.len() {
            let copy_end = page_end.min(payload.len());
            copy_into_frame(frame.base, &payload[page_offset..copy_end]);
        }

        let mut flags = MappingFlags::USER_ACCESSIBLE;
        if page_offset < header.executable_limit {
            flags |= MappingFlags::EXECUTABLE;
        }
        if page_end > header.writable_offset {
            flags |= MappingFlags::WRITABLE;
        }
        mapper.map_page(
            header
                .image_base
                .offset((page_index as u64) * PAGE_SIZE_BYTES),
            frame,
            flags,
            frame_allocator,
        )?;
    }

    // Map declared companion library images (extended-header images only).
    let mut libraries = [LoadedLibraryRecord::EMPTY; MAX_FLAT_DEPENDENCIES];
    let mut library_count = 0usize;
    if header.dependency_count > 0 {
        let resolve = crate::user::image_resolver().ok_or(LoadError::DependencyUnavailable)?;
        let mut cursor = align_up_u64(
            header.image_base.as_u64() + header.memory_size as u64,
            PAGE_SIZE_BYTES as u64,
        );
        for dep in header.dependencies() {
            let bytes = resolve(dep.image_id).ok_or(LoadError::DependencyUnavailable)?;
            let dep_header = parse_flat_image(bytes).map_err(|_| LoadError::DependencyInvalid)?;
            let base = if dep.base_offset_hint != 0 {
                header.image_base.offset(dep.base_offset_hint)
            } else {
                VirtualAddress::new(cursor)
            };
            if base.as_u64() % PAGE_SIZE_BYTES != 0
                || !image_window_contains(base.as_u64(), dep_header.memory_size as u64)
                || base.as_u64() + dep_header.memory_size as u64
                    > header.user_stack_top.as_u64() - (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES
            {
                return Err(LoadError::DependencyInvalid);
            }
            let dep_payload =
                &bytes[dep_header.header_len..dep_header.header_len + dep_header.file_size];
            let dep_pages = dep_header.memory_size.div_ceil(PAGE_SIZE_BYTES as usize);
            for page_index in 0..dep_pages {
                let page_offset = page_index * PAGE_SIZE_BYTES as usize;
                let page_end = page_offset + PAGE_SIZE_BYTES as usize;
                let frame = allocate_zeroed_frame(frame_allocator)?;
                if page_offset < dep_payload.len() {
                    let copy_end = page_end.min(dep_payload.len());
                    copy_into_frame(frame.base, &dep_payload[page_offset..copy_end]);
                }
                let mut flags = MappingFlags::USER_ACCESSIBLE;
                if (page_offset as u64) < dep_header.executable_limit as u64 {
                    flags |= MappingFlags::EXECUTABLE;
                }
                if (page_offset as u64) >= dep_header.writable_offset as u64 {
                    flags |= MappingFlags::WRITABLE;
                }
                mapper.map_page(
                    base.offset((page_index as u64) * PAGE_SIZE_BYTES),
                    frame,
                    flags,
                    frame_allocator,
                )?;
            }
            libraries[library_count] = LoadedLibraryRecord {
                image_id: dep.image_id,
                base,
                mapped_bytes: dep_header.memory_size,
            };
            library_count += 1;
            cursor = align_up_u64(
                base.as_u64() + dep_header.memory_size as u64,
                PAGE_SIZE_BYTES as u64,
            );
        }
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
        file_size: header.file_size,
        mapped_image_bytes: header.memory_size,
        user_stack_top: header.user_stack_top,
        mapped_stack_bytes: USER_STACK_PAGES * PAGE_SIZE_BYTES as usize,
        stack_executable: false,
        libraries,
        library_count,
    })
}

pub fn load_image(
    image: &[u8],
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
    expected_machine: ElfMachine,
    user_stack_top: VirtualAddress,
) -> Result<LoadedUserImage, LoadError> {
    if image.len() >= flat_image_magic().len()
        && image[..flat_image_magic().len()] == flat_image_magic()
    {
        return load_flat_image(image, mapper, frame_allocator);
    }
    if image.len() >= ELF_MAGIC.len() && image[..ELF_MAGIC.len()] == ELF_MAGIC {
        return load_elf64_image(
            image,
            mapper,
            frame_allocator,
            expected_machine,
            user_stack_top,
        );
    }
    Err(LoadError::UnsupportedFormat)
}

fn load_elf64_image(
    image: &[u8],
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
    expected_machine: ElfMachine,
    user_stack_top: VirtualAddress,
) -> Result<LoadedUserImage, LoadError> {
    if image.len() < 64 {
        return Err(LoadError::Truncated);
    }
    if image[..4] != ELF_MAGIC {
        return Err(LoadError::InvalidMagic);
    }
    if image[4] != ELF_CLASS_64 || image[5] != ELF_DATA_LSB || image[6] != ELF_VERSION_CURRENT {
        return Err(LoadError::UnsupportedAbi);
    }
    let elf_type = read_u16_le(image, 16)?;
    let machine = read_u16_le(image, 18)?;
    let entry = read_u64_le(image, 24)?;
    let phoff = read_u64_le(image, 32)? as usize;
    let phentsize = read_u16_le(image, 54)? as usize;
    let phnum = read_u16_le(image, 56)? as usize;
    if elf_type != ELF_TYPE_EXEC || machine != expected_machine.as_u16() {
        return Err(if machine != expected_machine.as_u16() {
            LoadError::UnsupportedMachine
        } else {
            LoadError::UnsupportedAbi
        });
    }
    if phentsize != ELF_PROGRAM_HEADER_LEN || phnum == 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    if !image_window_contains(
        user_stack_top
            .as_u64()
            .saturating_sub((USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES),
        (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES,
    ) {
        return Err(LoadError::UnsupportedHeader);
    }

    let mut image_base = u64::MAX;
    let mut image_end = 0u64;
    let mut gnu_stack_executable = false;
    let page_size = PAGE_SIZE_BYTES as usize;

    for index in 0..phnum {
        let header = phoff
            .checked_add(index * phentsize)
            .ok_or(LoadError::Truncated)?;
        let program = image
            .get(header..header + phentsize)
            .ok_or(LoadError::Truncated)?;
        let segment_type = read_u32_le(program, 0)?;
        if segment_type == ELF_SEGMENT_GNU_STACK {
            gnu_stack_executable = read_u32_le(program, 4)? & ELF_FLAG_EXECUTE != 0;
            continue;
        }
        if segment_type != ELF_SEGMENT_LOAD {
            continue;
        }
        let flags = read_u32_le(program, 4)?;
        let file_offset = read_u64_le(program, 8)? as usize;
        let virtual_address = read_u64_le(program, 16)?;
        let file_size = read_u64_le(program, 32)? as usize;
        let memory_size = read_u64_le(program, 40)? as usize;
        if virtual_address % PAGE_SIZE_BYTES != 0 || file_size > memory_size {
            return Err(LoadError::AddressAlignment);
        }
        if !image_window_contains(virtual_address, memory_size as u64) {
            return Err(LoadError::UnsupportedHeader);
        }
        let payload = image
            .get(file_offset..file_offset + file_size)
            .ok_or(LoadError::Truncated)?;
        let segment_pages = memory_size.div_ceil(page_size);
        image_base = image_base.min(virtual_address);
        image_end = image_end.max(virtual_address + memory_size as u64);

        for page_index in 0..segment_pages {
            let page_offset = page_index * page_size;
            let page_end = page_offset.saturating_add(page_size);
            let frame = allocate_zeroed_frame(frame_allocator)?;
            if page_offset < payload.len() {
                let copy_end = page_end.min(payload.len());
                copy_into_frame(frame.base, &payload[page_offset..copy_end]);
            }
            let mut mapping = MappingFlags::USER_ACCESSIBLE;
            if flags & ELF_FLAG_WRITE != 0 {
                mapping |= MappingFlags::WRITABLE;
            }
            if flags & ELF_FLAG_EXECUTE != 0 {
                mapping |= MappingFlags::EXECUTABLE;
            }
            mapper.map_page(
                VirtualAddress::new(virtual_address + (page_index as u64) * PAGE_SIZE_BYTES),
                frame,
                mapping,
                frame_allocator,
            )?;
        }
    }

    if image_base == u64::MAX || image_end <= image_base {
        return Err(LoadError::UnsupportedHeader);
    }

    let stack_bottom = VirtualAddress::new(
        user_stack_top.as_u64() - ((USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES),
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
        entry_point: VirtualAddress::new(entry),
        image_base: VirtualAddress::new(image_base),
        file_size: image.len(),
        mapped_image_bytes: (image_end - image_base) as usize,
        user_stack_top,
        mapped_stack_bytes: USER_STACK_PAGES * PAGE_SIZE_BYTES as usize,
        stack_executable: gnu_stack_executable,
        libraries: [LoadedLibraryRecord::EMPTY; MAX_FLAT_DEPENDENCIES],
        library_count: 0,
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

fn read_u16_le(image: &[u8], offset: usize) -> Result<u16, LoadError> {
    let bytes = image
        .get(offset..offset + 2)
        .ok_or(LoadError::Truncated)?
        .try_into()
        .map_err(|_| LoadError::Truncated)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u64_le(image: &[u8], offset: usize) -> Result<u64, LoadError> {
    let bytes = image
        .get(offset..offset + 8)
        .ok_or(LoadError::Truncated)?
        .try_into()
        .map_err(|_| LoadError::Truncated)?;
    Ok(u64::from_le_bytes(bytes))
}
