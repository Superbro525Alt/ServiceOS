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
/// `ET_DYN`: position-independent image; every p_vaddr/e_entry is relative to
/// a runtime-chosen load base.
const ELF_TYPE_DYN: u16 = 3;
const ELF_MACHINE_X86_64: u16 = 62;
const ELF_MACHINE_AARCH64: u16 = 183;
const ELF_PROGRAM_HEADER_LEN: usize = 56;
const ELF_SEGMENT_LOAD: u32 = 1;
/// `PT_DYNAMIC`: locates the dynamic section used to find `.rela.dyn`.
const ELF_SEGMENT_DYNAMIC: u32 = 2;
/// `PT_GNU_STACK`: presence records the stack execute policy for the image.
const ELF_SEGMENT_GNU_STACK: u32 = 0x6474_e551;
const ELF_FLAG_EXECUTE: u32 = 1;
const ELF_FLAG_WRITE: u32 = 2;

/// Dynamic-section tags (`Elf64_Dyn.d_tag`) consulted for relocation data.
const DYNAMIC_TAG_NULL: u64 = 0;
const DYNAMIC_TAG_RELA: u64 = 7;
const DYNAMIC_TAG_RELASZ: u64 = 8;
const DYNAMIC_ENTRY_LEN: usize = 16;
/// `R_X86_64_RELATIVE`: the only relocation class this loader applies. The
/// stored word becomes `load_base + r_addend` at `load_base + r_offset`.
const ELF_RELOC_RELATIVE: u64 = 8;
const ELF_RELA_ENTRY_LEN: usize = 24;
/// Upper bound on PT_LOAD segments tracked while mapping an image and
/// resolving relocation targets back to physical frames.
const MAX_ELF_LOAD_SEGMENTS: usize = 16;

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

/// One parsed `PT_LOAD` program header.
#[derive(Clone, Copy)]
struct ElfLoadSegment {
    flags: u32,
    file_offset: usize,
    virtual_address: u64,
    file_size: usize,
    memory_size: usize,
}

/// One PT_LOAD already copied into freshly allocated frames, used to translate
/// relocation targets back to the physical frames backing them.
#[derive(Clone, Copy)]
struct MappedLoadSegment {
    virtual_start: u64,
    memory_size: u64,
    frame_base: PhysicalAddress,
}

/// Deterministic load-base selection for `ET_DYN` images: every image loads at
/// the bottom of the user image window, which is page-aligned by definition
/// and below the stack region every caller reserves under `stack_bottom`.
fn choose_pie_load_base(highest_image_end: u64, stack_bottom: u64) -> Result<u64, LoadError> {
    if highest_image_end == 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    let base = USER_IMAGE_WINDOW_START;
    let limit = stack_bottom.saturating_sub(base);
    if highest_image_end > limit {
        return Err(LoadError::UnsupportedHeader);
    }
    Ok(base)
}

/// Walk a PT_DYNAMIC payload and extract the `.rela.dyn` location
/// (`DT_RELA`, in image-relative vaddrs) and byte size (`DT_RELASZ`).
fn parse_dynamic_rela_info(dynamic: &[u8]) -> Result<Option<(u64, u64)>, LoadError> {
    if dynamic.len() % DYNAMIC_ENTRY_LEN != 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    let mut rela_address: Option<u64> = None;
    let mut rela_size: Option<u64> = None;
    for entry_index in 0..dynamic.len() / DYNAMIC_ENTRY_LEN {
        let base = entry_index * DYNAMIC_ENTRY_LEN;
        let tag = read_u64_le(dynamic, base)?;
        if tag == DYNAMIC_TAG_NULL {
            break;
        }
        match tag {
            DYNAMIC_TAG_RELA => rela_address = Some(read_u64_le(dynamic, base + 8)?),
            DYNAMIC_TAG_RELASZ => rela_size = Some(read_u64_le(dynamic, base + 8)?),
            _ => {}
        }
    }
    match (rela_address, rela_size) {
        (Some(address), Some(size)) => {
            if size % ELF_RELA_ENTRY_LEN as u64 != 0 {
                return Err(LoadError::UnsupportedHeader);
            }
            Ok(Some((address, size)))
        }
        (None, None) => Ok(None),
        _ => Err(LoadError::UnsupportedHeader),
    }
}

/// Apply a raw `.rela.dyn` table. Only `R_X86_64_RELATIVE` entries are
/// supported: each stores `load_base + r_addend` at vaddr
/// `load_base + r_offset`. The caller supplies the bounds-checked writer for
/// the destination address space, keeping this logic host-testable against a
/// plain byte buffer.
fn apply_relative_relocations(
    rela_table: &[u8],
    load_base: u64,
    mut write_word: impl FnMut(u64, u64) -> Result<(), LoadError>,
) -> Result<usize, LoadError> {
    if rela_table.len() % ELF_RELA_ENTRY_LEN != 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    let entry_count = rela_table.len() / ELF_RELA_ENTRY_LEN;
    for entry_index in 0..entry_count {
        let base = entry_index * ELF_RELA_ENTRY_LEN;
        let r_offset = read_u64_le(rela_table, base)?;
        let r_info = read_u64_le(rela_table, base + 8)?;
        // Sign-extend the addend exactly like the hardware would read it.
        let addend = read_u64_le(rela_table, base + 16)?; // i64 bit pattern
        if r_info & 0xffff_ffff != ELF_RELOC_RELATIVE {
            return Err(LoadError::UnsupportedRelocation);
        }
        write_word(
            r_offset.wrapping_add(load_base),
            load_base.wrapping_add(addend),
        )?;
    }
    Ok(entry_count)
}

/// Write one relocated word through the physical frame that backs `target`.
/// The target must land fully inside a mapped PT_LOAD; anything else is
/// rejected rather than written.
fn write_mapped_word(
    segments: &[MappedLoadSegment],
    segment_count: usize,
    target: u64,
    value: u64,
) -> Result<(), LoadError> {
    for segment in &segments[..segment_count] {
        if let Some(offset) = target.checked_sub(segment.virtual_start) {
            if offset < segment.memory_size
                && offset.checked_add(8).is_some_and(|end| end <= segment.memory_size)
            {
                unsafe {
                    ((segment.frame_base.as_u64() + offset) as *mut u64).write_unaligned(value);
                }
                return Ok(());
            }
        }
    }
    Err(LoadError::UnsupportedRelocation)
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
    if machine != expected_machine.as_u16() {
        return Err(LoadError::UnsupportedMachine);
    }
    let position_independent = match elf_type {
        ELF_TYPE_EXEC => false,
        ELF_TYPE_DYN => true,
        _ => return Err(LoadError::UnsupportedAbi),
    };
    if phentsize != ELF_PROGRAM_HEADER_LEN || phnum == 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    let stack_bottom_value =
        user_stack_top.as_u64() - ((USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES);
    if !image_window_contains(stack_bottom_value, (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES) {
        return Err(LoadError::UnsupportedHeader);
    }

    let mut loads = [ElfLoadSegment {
        flags: 0,
        file_offset: 0,
        virtual_address: 0,
        file_size: 0,
        memory_size: 0,
    }; MAX_ELF_LOAD_SEGMENTS];
    let mut load_count = 0usize;
    let mut dynamic_range: Option<(usize, usize)> = None;
    let mut gnu_stack_executable = false;

    for index in 0..phnum {
        let header = phoff
            .checked_add(index * phentsize)
            .ok_or(LoadError::Truncated)?;
        let program = image
            .get(header..header + phentsize)
            .ok_or(LoadError::Truncated)?;
        let segment_type = read_u32_le(program, 0)?;
        match segment_type {
            ELF_SEGMENT_GNU_STACK => {
                gnu_stack_executable = read_u32_le(program, 4)? & ELF_FLAG_EXECUTE != 0;
            }
            ELF_SEGMENT_DYNAMIC => {
                if dynamic_range.is_some() {
                    return Err(LoadError::UnsupportedHeader);
                }
                let dyn_offset = read_u64_le(program, 8)? as usize;
                let dyn_size = read_u64_le(program, 32)? as usize;
                image.get(dyn_offset..dyn_offset.checked_add(dyn_size).ok_or(LoadError::Truncated)?)
                    .ok_or(LoadError::Truncated)?;
                dynamic_range = Some((dyn_offset, dyn_size));
            }
            ELF_SEGMENT_LOAD => {
                if load_count == MAX_ELF_LOAD_SEGMENTS {
                    return Err(LoadError::UnsupportedHeader);
                }
                loads[load_count] = ElfLoadSegment {
                    flags: read_u32_le(program, 4)?,
                    file_offset: read_u64_le(program, 8)? as usize,
                    virtual_address: read_u64_le(program, 16)?,
                    file_size: read_u64_le(program, 32)? as usize,
                    memory_size: read_u64_le(program, 40)? as usize,
                };
                load_count += 1;
            }
            _ => {}
        }
    }

    if load_count == 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    for segment in &loads[..load_count] {
        if segment.virtual_address % PAGE_SIZE_BYTES != 0 || segment.file_size > segment.memory_size
        {
            return Err(LoadError::AddressAlignment);
        }
        if image
            .get(segment.file_offset..segment.file_offset + segment.file_size)
            .is_none()
        {
            return Err(LoadError::Truncated);
        }
    }

    // ET_DYN picks a deterministic page-aligned base at the bottom of the
    // user image window; ET_EXEC keeps its fixed link-time addresses.
    let load_base = if position_independent {
        let highest_end = loads[..load_count]
            .iter()
            .map(|segment| segment.virtual_address.saturating_add(segment.memory_size as u64))
            .max()
            .unwrap_or(0);
        choose_pie_load_base(highest_end, stack_bottom_value)?
    } else {
        0
    };

    let page_size = PAGE_SIZE_BYTES as usize;
    let mut image_base = u64::MAX;
    let mut image_end = 0u64;
    let mut mapped_segments = [MappedLoadSegment {
        virtual_start: 0,
        memory_size: 0,
        frame_base: PhysicalAddress::new(0),
    }; MAX_ELF_LOAD_SEGMENTS];

    for (segment_index, segment) in loads[..load_count].iter().enumerate() {
        let virtual_start = if position_independent {
            load_base.checked_add(segment.virtual_address).ok_or(LoadError::UnsupportedHeader)?
        } else {
            segment.virtual_address
        };
        if !image_window_contains(virtual_start, segment.memory_size as u64) {
            return Err(LoadError::UnsupportedHeader);
        }
        if position_independent && virtual_start + segment.memory_size as u64 > stack_bottom_value {
            return Err(LoadError::UnsupportedHeader);
        }
        let payload = image
            .get(segment.file_offset..segment.file_offset + segment.file_size)
            .ok_or(LoadError::Truncated)?;
        let segment_pages = segment.memory_size.div_ceil(page_size);
        image_base = image_base.min(virtual_start);
        image_end = image_end.max(virtual_start + segment.memory_size as u64);

        let mut segment_frames: Option<(PhysicalAddress, usize)> = None;
        for page_index in 0..segment_pages {
            let page_offset = page_index * page_size;
            let page_end = page_offset.saturating_add(page_size);
            let frame = allocate_zeroed_frame(frame_allocator)?;
            if page_index == 0 {
                segment_frames = Some((frame.base, segment_pages));
            }
            if page_offset < payload.len() {
                let copy_end = page_end.min(payload.len());
                copy_into_frame(frame.base, &payload[page_offset..copy_end]);
            }
            let mut mapping = MappingFlags::USER_ACCESSIBLE;
            if segment.flags & ELF_FLAG_WRITE != 0 {
                mapping |= MappingFlags::WRITABLE;
            }
            if segment.flags & ELF_FLAG_EXECUTE != 0 {
                mapping |= MappingFlags::EXECUTABLE;
            }
            mapper.map_page(
                VirtualAddress::new(virtual_start + (page_index as u64) * PAGE_SIZE_BYTES),
                frame,
                mapping,
                frame_allocator,
            )?;
        }
        if let Some((frame_base, pages)) = segment_frames {
            mapped_segments[segment_index] = MappedLoadSegment {
                virtual_start,
                memory_size: (pages * page_size) as u64,
                frame_base,
            };
        }
    }

    // Apply `.rela.dyn` R_X86_64_RELATIVE fixups discovered through
    // PT_DYNAMIC. Position-independent images are only executable once these
    // absolute addresses have been biased by the chosen load base.
    if position_independent {
        if let Some((dyn_offset, dyn_size)) = dynamic_range {
            let dynamic_bytes = &image[dyn_offset..dyn_offset + dyn_size];
            if let Some((rela_vaddr, rela_size)) = parse_dynamic_rela_info(dynamic_bytes)? {
                let rela_bytes = locate_file_range(
                    image,
                    &loads[..load_count],
                    rela_vaddr,
                    rela_size as usize,
                )
                .ok_or(LoadError::UnsupportedRelocation)?;
                apply_relative_relocations(rela_bytes, load_base, |target, value| {
                    write_mapped_word(&mapped_segments, load_count, target, value)
                })?;
            }
        }
    }

    let entry_point = if position_independent {
        load_base
            .checked_add(entry)
            .ok_or(LoadError::UnsupportedHeader)?
    } else {
        entry
    };

    for page_index in 0..USER_STACK_PAGES {
        let frame = allocate_zeroed_frame(frame_allocator)?;
        mapper.map_page(
            VirtualAddress::new(stack_bottom_value + (page_index as u64) * PAGE_SIZE_BYTES),
            frame,
            MappingFlags::WRITABLE | MappingFlags::USER_ACCESSIBLE,
            frame_allocator,
        )?;
    }

    Ok(LoadedUserImage {
        entry_point: VirtualAddress::new(entry_point),
        image_base: VirtualAddress::new(if position_independent { load_base } else { image_base }),
        file_size: image.len(),
        mapped_image_bytes: (image_end - image_base) as usize,
        user_stack_top,
        mapped_stack_bytes: USER_STACK_PAGES * PAGE_SIZE_BYTES as usize,
        stack_executable: gnu_stack_executable,
        libraries: [LoadedLibraryRecord::EMPTY; MAX_FLAT_DEPENDENCIES],
        library_count: 0,
    })
}

/// Translate an image-relative vaddr range into a slice of the ELF file using
/// the p_vaddr/p_offset correspondence of the containing PT_LOAD.
fn locate_file_range<'a>(
    image: &'a [u8],
    segments: &[ElfLoadSegment],
    virtual_address: u64,
    length: usize,
) -> Option<&'a [u8]> {
    for segment in segments {
        let seg_start = segment.virtual_address;
        let seg_end = seg_start.checked_add(segment.file_size as u64)?;
        if virtual_address >= seg_start && virtual_address < seg_end {
            let offset_in_segment = (virtual_address - seg_start) as usize;
            let file_start = segment.file_offset.checked_add(offset_in_segment)?;
            let file_end = file_start.checked_add(length)?;
            if file_end <= segment.file_offset + segment.file_size {
                return image.get(file_start..file_end);
            }
        }
    }
    None
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

#[cfg(test)]
mod pie_tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    const TEST_BASE: u64 = 0x0000_4000_0000_0000;
    const TEST_STACK_TOP: u64 = USER_SPACE_END.as_u64() - 0x10_0000;

    fn push_u16(buffer: &mut Vec<u8>, value: u16) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(buffer: &mut Vec<u8>, value: u32) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(buffer: &mut Vec<u8>, value: u64) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }

    fn program_header(
        segment_type: u32,
        flags: u32,
        file_offset: u64,
        virtual_address: u64,
        file_size: u64,
        memory_size: u64,
    ) -> Vec<u8> {
        let mut header = Vec::new();
        push_u32(&mut header, segment_type);
        push_u32(&mut header, flags);
        push_u64(&mut header, file_offset);
        push_u64(&mut header, virtual_address);
        push_u64(&mut header, virtual_address);
        push_u64(&mut header, file_size);
        push_u64(&mut header, memory_size);
        push_u64(&mut header, PAGE_SIZE_BYTES as u64);
        header
    }

    /// Golden minimal ET_DYN: RX text LOAD at vaddr 0x1000, RW data LOAD at
    /// vaddr 0x4000 carrying PT_DYNAMIC and a `.rela.dyn` of three
    /// R_X86_64_RELATIVE entries.
    fn golden_minimal_et_dyn() -> Vec<u8> {
        const RELA_FILE_OFFSET: usize = 0x2200;
        const RELA_VADDR: u64 = 0x4200;
        const DYN_VADDR: u64 = 0x4100;

        let mut image = Vec::new();
        // ELF header.
        image.extend_from_slice(&ELF_MAGIC);
        image.push(ELF_CLASS_64);
        image.push(ELF_DATA_LSB);
        image.push(ELF_VERSION_CURRENT);
        image.extend_from_slice(&[0; 9]);
        push_u16(&mut image, ELF_TYPE_DYN);
        push_u16(&mut image, ELF_MACHINE_X86_64);
        push_u32(&mut image, 1); // e_version
        push_u64(&mut image, 0x1040); // e_entry, base-relative
        push_u64(&mut image, 64); // e_phoff
        push_u64(&mut image, 0); // e_shoff
        push_u32(&mut image, 0); // e_flags
        push_u16(&mut image, 64); // e_ehsize
        push_u16(&mut image, ELF_PROGRAM_HEADER_LEN as u16);
        push_u16(&mut image, 3); // e_phnum
        push_u16(&mut image, 0);
        push_u16(&mut image, 0);
        push_u16(&mut image, 0);

        let mut headers = Vec::new();
        headers.extend(program_header(ELF_SEGMENT_LOAD, ELF_FLAG_EXECUTE, 0x1000, 0x1000, 8, 0x2000));
        headers.extend(program_header(
            ELF_SEGMENT_LOAD,
            ELF_FLAG_WRITE,
            0x2000,
            0x4000,
            0x300,
            0x1000,
        ));
        headers.extend(program_header(
            ELF_SEGMENT_DYNAMIC,
            ELF_FLAG_WRITE,
            0x2100,
            DYN_VADDR,
            48,
            48,
        ));
        assert_eq!(headers.len(), 3 * ELF_PROGRAM_HEADER_LEN);
        image.extend_from_slice(&headers);
        assert_eq!(image.len(), 64 + 168);

        // Payload padding up to the RW segment file area.
        while image.len() < 0x2000 {
            image.push(0);
        }
        while image.len() < 0x2100 {
            image.push(0xEE);
        }
        // PT_DYNAMIC payload: DT_RELA, DT_RELASZ(3 entries), DT_NULL.
        let mut dynamic = Vec::new();
        push_u64(&mut dynamic, DYNAMIC_TAG_RELA);
        push_u64(&mut dynamic, RELA_VADDR);
        push_u64(&mut dynamic, DYNAMIC_TAG_RELASZ);
        push_u64(&mut dynamic, 72);
        push_u64(&mut dynamic, DYNAMIC_TAG_NULL);
        push_u64(&mut dynamic, 0);
        assert_eq!(dynamic.len(), 48);
        image.extend_from_slice(&dynamic);
        while image.len() < RELA_FILE_OFFSET {
            image.push(0);
        }
        // `.rela.dyn`: three R_X86_64_RELATIVE fixups.
        for (offset, addend) in [(0x4100u64, 0x1234i64), (0x4110, -8), (0x4140, 0)] {
            let mut entry = Vec::new();
            push_u64(&mut entry, offset);
            push_u64(&mut entry, ELF_RELOC_RELATIVE);
            push_u64(&mut entry, addend as u64);
            image.extend_from_slice(&entry);
        }
        image
    }

    #[test]
    fn relative_relocations_apply_into_byte_buffer() {
        let image = golden_minimal_et_dyn();
        assert_eq!(image.len(), 0x2248);
        let loads = [
            ElfLoadSegment { flags: ELF_FLAG_EXECUTE, file_offset: 0x1000, virtual_address: 0x1000, file_size: 8, memory_size: 0x2000 },
            ElfLoadSegment { flags: ELF_FLAG_WRITE, file_offset: 0x2000, virtual_address: 0x4000, file_size: 0x300, memory_size: 0x1000 },
        ];
        let dynamic_bytes = &image[0x2100..0x2100 + 48];
        let (rela_vaddr, rela_size) = parse_dynamic_rela_info(dynamic_bytes)
            .expect("rela info parses")
            .expect("rela table present");
        assert_eq!(rela_vaddr, 0x4200);
        assert_eq!(rela_size, 72);

        let load_base = choose_pie_load_base(0x5000, TEST_STACK_TOP).expect("base chosen");
        assert_eq!(load_base, USER_IMAGE_WINDOW_START);
        assert_eq!(load_base % PAGE_SIZE_BYTES, 0);

        let rela_bytes = locate_file_range(
            image.as_slice(),
            &loads,
            rela_vaddr,
            rela_size as usize,
        )
        .expect("rela table resolves into the file");

        // Byte-buffer stand-in for the mapped image [load_base, load_base+0x5000).
        let mut memory = vec![0u8; 0x5000];
        let applied = apply_relative_relocations(rela_bytes, load_base, |target, value| {
            let index = target
                .checked_sub(load_base)
                .ok_or(LoadError::UnsupportedRelocation)?;
            let end = index.checked_add(8).ok_or(LoadError::UnsupportedRelocation)? as usize;
            if end > memory.len() {
                return Err(LoadError::UnsupportedRelocation);
            }
            memory[index as usize..end].copy_from_slice(&value.to_le_bytes());
            Ok(())
        })
        .expect("all relocations apply");
        assert_eq!(applied, 3);

        let read_word = |vaddr: u64| -> u64 {
            let index = (vaddr - load_base) as usize;
            u64::from_le_bytes(memory[index..index + 8].try_into().unwrap())
        };
        assert_eq!(read_word(load_base + 0x4100), load_base + 0x1234);
        assert_eq!(read_word(load_base + 0x4110), (load_base as i64 - 8) as u64);
        assert_eq!(read_word(load_base + 0x4140), load_base);
    }

    #[test]
    fn unsupported_relocation_types_are_rejected() {
        let mut table = Vec::new();
        push_u64(&mut table, 0x4100);
        push_u64(&mut table, 1); // R_X86_64_64 — not supported
        push_u64(&mut table, 0);
        let mut writes = 0usize;
        let result = apply_relative_relocations(&table, TEST_BASE, |_target, _value| {
            writes += 1;
            Ok(())
        });
        assert_eq!(result, Err(LoadError::UnsupportedRelocation));
        assert_eq!(writes, 0);
    }

    #[test]
    fn out_of_bounds_relocation_targets_are_rejected() {
        let mut table = Vec::new();
        // Target sits past the end of the simulated mapping.
        push_u64(&mut table, 0x4ff8);
        push_u64(&mut table, ELF_RELOC_RELATIVE);
        push_u64(&mut table, 0);
        let memory_len = 0x1000u64;
        let result = apply_relative_relocations(&table, TEST_BASE, |target, _value| {
            let index = (target - TEST_BASE) as usize;
            if index
                .checked_add(8)
                .map_or(true, |end| end > memory_len as usize)
            {
                return Err(LoadError::UnsupportedRelocation);
            }
            Ok(())
        });
        assert_eq!(result, Err(LoadError::UnsupportedRelocation));
    }

    #[test]
    fn malformed_dynamic_blocks_are_rejected() {
        assert_eq!(parse_dynamic_rela_info(&[]), Ok(None));

        // DT_RELA without DT_RELASZ is malformed.
        let mut dynamic = Vec::new();
        push_u64(&mut dynamic, DYNAMIC_TAG_RELA);
        push_u64(&mut dynamic, 0x4300);
        push_u64(&mut dynamic, DYNAMIC_TAG_NULL);
        push_u64(&mut dynamic, 0);
        assert_eq!(
            parse_dynamic_rela_info(&dynamic),
            Err(LoadError::UnsupportedHeader)
        );

        // A non-multiple-of-24 size is malformed.
        let mut dynamic = Vec::new();
        push_u64(&mut dynamic, DYNAMIC_TAG_RELA);
        push_u64(&mut dynamic, 0x4300);
        push_u64(&mut dynamic, DYNAMIC_TAG_RELASZ);
        push_u64(&mut dynamic, 25);
        push_u64(&mut dynamic, DYNAMIC_TAG_NULL);
        push_u64(&mut dynamic, 0);
        assert_eq!(
            parse_dynamic_rela_info(&dynamic),
            Err(LoadError::UnsupportedHeader)
        );

        // Entries after DT_NULL are ignored.
        let mut dynamic = Vec::new();
        push_u64(&mut dynamic, DYNAMIC_TAG_NULL);
        push_u64(&mut dynamic, 0);
        push_u64(&mut dynamic, DYNAMIC_TAG_RELASZ);
        push_u64(&mut dynamic, 999);
        assert_eq!(parse_dynamic_rela_info(&dynamic), Ok(None));
    }

    #[test]
    fn pie_load_base_is_deterministic_and_page_aligned() {
        let stack_bottom = TEST_STACK_TOP - (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES;
        assert_eq!(choose_pie_load_base(0x5000, stack_bottom), Ok(USER_IMAGE_WINDOW_START));
        // Span reaching the stack region must be refused.
        let too_big = stack_bottom - USER_IMAGE_WINDOW_START + 1;
        assert_eq!(
            choose_pie_load_base(too_big, stack_bottom),
            Err(LoadError::UnsupportedHeader)
        );
        assert_eq!(
            choose_pie_load_base(0, stack_bottom),
            Err(LoadError::UnsupportedHeader)
        );
    }

    #[test]
    fn rela_tables_must_be_entry_aligned() {
        let mut table = vec![0u8; 23];
        assert_eq!(
            apply_relative_relocations(&table, TEST_BASE, |_, _| Ok(())),
            Err(LoadError::UnsupportedHeader)
        );
        table.resize(24, 0);
        // Single RELATIVE entry with zero offset/addend applies cleanly.
        table[8] = 8; // r_info: R_X86_64_RELATIVE
        assert_eq!(
            apply_relative_relocations(&table, TEST_BASE, |_, _| Ok(())),
            Ok(1)
        );
    }
}
