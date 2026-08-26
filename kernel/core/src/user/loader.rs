use crate::memory::{
    EarlyFrameAllocator, Frame, MappingFlags, PAGE_SIZE_BYTES, PageMapper, PhysicalAddress,
    USER_SPACE_END, VirtualAddress,
};

use alloc::boxed::Box;
use alloc::vec::Vec;

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
/// `DT_PLTRELSZ`: byte size of the `.rela.plt` table.
const DYNAMIC_TAG_PLTRELSZ: u64 = 2;
/// `DT_HASH`: SysV symbol-hash table. The minimum (and currently only) hash
/// format the loader supports; it supplies `nchain`, bounding dynsym reads.
const DYNAMIC_TAG_HASH: u64 = 4;
const DYNAMIC_TAG_STRTAB: u64 = 5;
const DYNAMIC_TAG_SYMTAB: u64 = 6;
const DYNAMIC_TAG_RELA: u64 = 7;
const DYNAMIC_TAG_RELASZ: u64 = 8;
/// `DT_STRSZ`: byte size of the dynamic string table.
const DYNAMIC_TAG_STRSZ: u64 = 10;
/// `DT_JMPREL`: address of the `.rela.plt` (JUMP_SLOT) relocation table.
const DYNAMIC_TAG_JMPREL: u64 = 23;
const DYNAMIC_ENTRY_LEN: usize = 16;
/// `R_X86_64_GLOB_DAT`: GOT slot filled with a resolved symbol address.
const ELF_RELOC_GLOB_DAT: u64 = 6;
/// `R_X86_64_JUMP_SLOT`: PLT/GOT slot filled with a resolved symbol address.
const ELF_RELOC_JUMP_SLOT: u64 = 7;
/// `R_X86_64_RELATIVE`: the stored word becomes `load_base + r_addend` at
/// `load_base + r_offset`.
const ELF_RELOC_RELATIVE: u64 = 8;
const ELF_RELA_ENTRY_LEN: usize = 24;
/// `Elf64_Sym` entry length in `.dynsym`.
const ELF_SYMBOL_ENTRY_LEN: usize = 24;
/// Symbol bindings eligible for export registration.
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;
/// `SHN_UNDEF`: symbol must be resolved against another module's exports.
const SHN_UNDEF: u16 = 0;
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
    expected_machine: ElfMachine,
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
    // A dependency may be a flat image (as before) or an ELF64 `ET_DYN`
    // shared object: ELF companions have their segments mapped at the same
    // deterministic base, their exports registered into this load's symbol
    // namespace, and their relocations applied once every image is mapped.
    let mut libraries = [LoadedLibraryRecord::EMPTY; MAX_FLAT_DEPENDENCIES];
    let mut library_count = 0usize;
    let mut pending_modules: Vec<PendingElfModule> = Vec::new();
    let mut namespace = Box::new(SymbolNamespace::EMPTY);
    if header.dependency_count > 0 {
        let resolve = crate::user::image_resolver().ok_or(LoadError::DependencyUnavailable)?;
        let mut cursor = align_up_u64(
            header.image_base.as_u64() + header.memory_size as u64,
            PAGE_SIZE_BYTES as u64,
        );
        for dep in header.dependencies() {
            let bytes = resolve(dep.image_id).ok_or(LoadError::DependencyUnavailable)?;
            let (base, mapped_bytes) =
                if bytes.len() >= ELF_MAGIC.len() && bytes[..ELF_MAGIC.len()] == ELF_MAGIC {
                    let plan = plan_elf_dependency(bytes, expected_machine)?;
                    let base = if dep.base_offset_hint != 0 {
                        VirtualAddress::new(dep.base_offset_hint + header.image_base.as_u64())
                    } else {
                        VirtualAddress::new(cursor)
                    };
                    if base.as_u64() % PAGE_SIZE_BYTES != 0
                        || !image_window_contains(base.as_u64(), plan.span)
                        || base.as_u64() + plan.span
                            > header.user_stack_top.as_u64()
                                - (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES
                    {
                        return Err(LoadError::DependencyInvalid);
                    }
                    let module = map_elf_dependency(
                        bytes,
                        &plan,
                        base,
                        mapper,
                        frame_allocator,
                        namespace.as_mut(),
                    )?;
                    pending_modules.push(module);
                    (base, plan.span as usize)
                } else {
                    let dep_header =
                        parse_flat_image(bytes).map_err(|_| LoadError::DependencyInvalid)?;
                    let base = if dep.base_offset_hint != 0 {
                        header.image_base.offset(dep.base_offset_hint)
                    } else {
                        VirtualAddress::new(cursor)
                    };
                    if base.as_u64() % PAGE_SIZE_BYTES != 0
                        || !image_window_contains(base.as_u64(), dep_header.memory_size as u64)
                        || base.as_u64() + dep_header.memory_size as u64
                            > header.user_stack_top.as_u64()
                                - (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES
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
                    (base, dep_header.memory_size)
                };
            libraries[library_count] = LoadedLibraryRecord {
                image_id: dep.image_id,
                base,
                mapped_bytes,
            };
            library_count += 1;
            cursor = align_up_u64(base.as_u64() + mapped_bytes as u64, PAGE_SIZE_BYTES as u64);
        }
    }

    // Every image is now mapped: apply each ELF companion's relocation
    // tables against the fully populated symbol namespace. Definitions were
    // registered in dependency declaration order, and the main image's own
    // load path registers last, so the main image can override dependencies.
    for module in pending_modules.iter() {
        module.apply_relocations(&namespace)?;
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
        return load_flat_image(image, mapper, frame_allocator, expected_machine);
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
                && offset
                    .checked_add(8)
                    .is_some_and(|end| end <= segment.memory_size)
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

/// Parsed subset of a `PT_DYNAMIC` payload: relocation tables plus the
/// locations of the dynamic symbol/string/hash tables (image-relative vaddrs).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ElfDynamicInfo {
    rela: Option<(u64, u64)>,
    jmprel: Option<(u64, u64)>,
    symtab_vaddr: Option<u64>,
    strtab_vaddr: Option<u64>,
    strtab_size: Option<u64>,
    hash_vaddr: Option<u64>,
}

/// Walk a PT_DYNAMIC payload collecting every tag the loader consumes.
/// Relocation address/size pairs must be present together or not at all;
/// a size that is not a multiple of the entry length is malformed.
fn parse_dynamic_info(dynamic: &[u8]) -> Result<ElfDynamicInfo, LoadError> {
    if dynamic.len() % DYNAMIC_ENTRY_LEN != 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    let mut info = ElfDynamicInfo::default();
    let mut pltrelsz: Option<u64> = None;
    for entry_index in 0..dynamic.len() / DYNAMIC_ENTRY_LEN {
        let base = entry_index * DYNAMIC_ENTRY_LEN;
        let tag = read_u64_le(dynamic, base)?;
        if tag == DYNAMIC_TAG_NULL {
            break;
        }
        let value = read_u64_le(dynamic, base + 8)?;
        match tag {
            DYNAMIC_TAG_RELA => info.rela = Some((value, info.rela.map_or(0, |(_, size)| size))),
            DYNAMIC_TAG_RELASZ => {
                info.rela = Some((info.rela.map_or(0, |(address, _)| address), value));
            }
            DYNAMIC_TAG_JMPREL => info.jmprel = Some((value, pltrelsz.unwrap_or(0))),
            DYNAMIC_TAG_PLTRELSZ => {
                pltrelsz = Some(value);
                info.jmprel = Some((info.jmprel.map_or(0, |(address, _)| address), value));
            }
            DYNAMIC_TAG_SYMTAB => info.symtab_vaddr = Some(value),
            DYNAMIC_TAG_STRTAB => info.strtab_vaddr = Some(value),
            DYNAMIC_TAG_STRSZ => info.strtab_size = Some(value),
            DYNAMIC_TAG_HASH => info.hash_vaddr = Some(value),
            _ => {}
        }
    }
    // Address without size (or vice versa) is malformed for either table.
    for table in [info.rela, info.jmprel] {
        if let Some((address, size)) = table {
            if address == 0 || size == 0 || size % ELF_RELA_ENTRY_LEN as u64 != 0 {
                return Err(LoadError::UnsupportedHeader);
            }
        }
    }
    Ok(info)
}

/// A SysV `DT_HASH` view over raw little-endian bytes (possibly unaligned in
/// the file image): bucket/chain arrays plus the total dynamic-symbol count
/// (`nchain`, by definition equal to the `.dynsym` entry count).
struct SysvHashView<'a> {
    buckets: &'a [u8],
    chains: &'a [u8],
    bucket_count: usize,
}

impl SysvHashView<'_> {
    fn bucket(&self, index: usize) -> Option<u32> {
        let base = index.checked_mul(4)?;
        read_u32_le(self.buckets, base).ok()
    }

    fn chain(&self, index: usize) -> u32 {
        self.chains
            .get(index * 4..index * 4 + 4)
            .and_then(|bytes| read_u32_le(bytes, 0).ok())
            .unwrap_or(0)
    }
}

/// The resolved dynamic symbol interface of one image: raw `.dynsym` bytes,
/// entry count, bounds-checked string table, and hash view when available.
struct ElfSymbolTable<'a> {
    symbols: &'a [u8],
    symbol_count: usize,
    strings: &'a [u8],
    hash: Option<SysvHashView<'a>>,
}

impl ElfSymbolTable<'_> {
    /// Classic System V symbol-table lookup by name (hash + bucket chain).
    /// Production loads resolve through the flattened namespace instead;
    /// this completes the DT_HASH contract and is covered by unit tests.
    #[cfg_attr(not(test), allow(dead_code))]
    fn lookup(&self, name: &[u8]) -> Option<u64> {
        let hash = self.hash.as_ref()?;
        if name.is_empty() || hash.bucket_count == 0 {
            return None;
        }
        let start = hash.bucket((elf_hash(name) as usize) % hash.bucket_count)?;
        let mut index = start as usize;
        while index != 0 {
            if index >= self.symbol_count {
                return None;
            }
            let Ok(symbol) = read_symbol_entry(self.symbols, index) else {
                return None;
            };
            if symbol.shndx != SHN_UNDEF
                && symbol_name_bytes(self.strings, symbol.name_offset) == Ok(name)
            {
                return Some(symbol.value);
            }
            index = hash.chain(index) as usize;
        }
        None
    }
}

/// One decoded `Elf64_Sym`.
struct ElfSymbol {
    name_offset: u32,
    binding: u8,
    shndx: u16,
    value: u64,
}

fn read_symbol_entry(symbols: &[u8], index: usize) -> Result<ElfSymbol, LoadError> {
    let base = index
        .checked_mul(ELF_SYMBOL_ENTRY_LEN)
        .ok_or(LoadError::UnsupportedHeader)?;
    if base + ELF_SYMBOL_ENTRY_LEN > symbols.len() {
        return Err(LoadError::UnsupportedHeader);
    }
    Ok(ElfSymbol {
        name_offset: read_u32_le(symbols, base)?,
        binding: symbols[base + 4] >> 4,
        shndx: read_u16_le(symbols, base + 6)?,
        value: read_u64_le(symbols, base + 8)?,
    })
}

/// NUL-terminated symbol name at `name_offset` inside the string table.
/// Reads are bounded by `DT_STRSZ`; a missing terminator is malformed.
fn symbol_name_bytes<'a>(strings: &'a [u8], name_offset: u32) -> Result<&'a [u8], LoadError> {
    let start = name_offset as usize;
    if start >= strings.len() {
        return Err(LoadError::UnsupportedHeader);
    }
    let tail = &strings[start..];
    match tail.iter().position(|&byte| byte == 0) {
        Some(end) => Ok(&tail[..end]),
        None => Err(LoadError::UnsupportedHeader),
    }
}

/// The System V ELF hash function used by `DT_HASH` bucket chains.
#[cfg_attr(not(test), allow(dead_code))]
fn elf_hash(name: &[u8]) -> u32 {
    let mut hash: u32 = 0;
    for &byte in name {
        hash = hash.wrapping_shl(4).wrapping_add(byte as u32);
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    hash
}

/// Locate and validate the dynsym/dynstr/DT_HASH tables declared by a
/// module's PT_DYNAMIC. Returns `None` when the module declares no complete
/// symbol interface; a partially-declared one is an error. `DT_HASH` is
/// required because it supplies `nchain`, bounding `.dynsym` reads.
fn resolve_symbol_tables<'a>(
    image: &'a [u8],
    loads: &[ElfLoadSegment],
    info: &ElfDynamicInfo,
) -> Result<Option<ElfSymbolTable<'a>>, LoadError> {
    let (Some(symtab_vaddr), Some(strtab_vaddr), Some(strtab_size), Some(hash_vaddr)) = (
        info.symtab_vaddr,
        info.strtab_vaddr,
        info.strtab_size,
        info.hash_vaddr,
    ) else {
        return Ok(None);
    };
    let strings = locate_file_range(image, loads, strtab_vaddr, strtab_size as usize)
        .ok_or(LoadError::UnsupportedRelocation)?;

    // Hash header first to learn nbucket/nchain, then locate the full table.
    let header =
        locate_file_range(image, loads, hash_vaddr, 8).ok_or(LoadError::UnsupportedRelocation)?;
    let nbucket = read_u32_le(header, 0)? as usize;
    let nchain = read_u32_le(header, 4)? as usize;
    if nbucket == 0 || nchain == 0 {
        return Err(LoadError::UnsupportedHeader);
    }
    let hash_bytes = locate_file_range(image, loads, hash_vaddr, 8 + (nbucket + nchain) * 4)
        .ok_or(LoadError::UnsupportedRelocation)?;
    let buckets_end = 8 + nbucket * 4;

    let symbols = locate_file_range(image, loads, symtab_vaddr, nchain * ELF_SYMBOL_ENTRY_LEN)
        .ok_or(LoadError::UnsupportedRelocation)?;
    Ok(Some(ElfSymbolTable {
        symbols,
        symbol_count: nchain,
        strings,
        hash: Some(SysvHashView {
            buckets: &hash_bytes[8..buckets_end],
            chains: &hash_bytes[buckets_end..],
            bucket_count: nbucket,
        }),
    }))
}

/// Fixed-capacity per-load symbol namespace. Registration walks dependencies
/// in declaration order with the main image registered last; a later strong
/// registration overrides an earlier weak one and later registrations override
/// earlier ones of equal strength, so the main image's definitions win.
const SYMBOL_NAMESPACE_CAPACITY: usize = 64;
/// Names longer than this are not registered (they can never resolve here).
const SYMBOL_NAME_MAX: usize = 40;

#[derive(Clone, Copy)]
struct SymbolEntry {
    name_len: u8,
    name: [u8; SYMBOL_NAME_MAX],
    address: u64,
    weak: bool,
}

struct SymbolNamespace {
    entries: [SymbolEntry; SYMBOL_NAMESPACE_CAPACITY],
    count: usize,
}

impl SymbolNamespace {
    const EMPTY: Self = Self {
        entries: [SymbolEntry {
            name_len: 0,
            name: [0; SYMBOL_NAME_MAX],
            address: 0,
            weak: false,
        }; SYMBOL_NAMESPACE_CAPACITY],
        count: 0,
    };

    fn find(&self, name: &[u8]) -> Option<usize> {
        self.entries[..self.count].iter().position(|entry| {
            entry.name_len as usize == name.len() && &entry.name[..name.len()] == name
        })
    }

    /// Register one export. Strong beats weak regardless of order; among
    /// equal strengths the latest registration wins (main registers last).
    fn insert(&mut self, name: &[u8], weak: bool, address: u64) -> Result<(), LoadError> {
        if name.is_empty() || name.len() > SYMBOL_NAME_MAX {
            return Ok(());
        }
        if let Some(index) = self.find(name) {
            let entry = &mut self.entries[index];
            if entry.weak && !weak {
                // A later strong definition overrides an earlier weak one.
                entry.weak = false;
                entry.address = address;
            } else if entry.weak == weak {
                // Among equal strengths the latest registration wins.
                entry.address = address;
            }
            return Ok(());
        }
        if self.count == SYMBOL_NAMESPACE_CAPACITY {
            return Err(LoadError::SymbolSpaceExhausted);
        }
        let entry = &mut self.entries[self.count];
        entry.name_len = name.len() as u8;
        entry.name[..name.len()].copy_from_slice(name);
        entry.address = address;
        entry.weak = weak;
        self.count += 1;
        Ok(())
    }

    fn lookup(&self, name: &[u8]) -> Option<u64> {
        self.find(name).map(|index| self.entries[index].address)
    }
}

/// Register every defined global/weak export of a module into the namespace,
/// discovering entries through its `DT_HASH` buckets with addresses biased by
/// the module's load base.
fn register_module_exports(
    table: &ElfSymbolTable,
    load_base: u64,
    namespace: &mut SymbolNamespace,
) -> Result<(), LoadError> {
    let Some(hash) = &table.hash else {
        return Ok(());
    };
    for bucket_index in 0..hash.bucket_count {
        let mut index = hash.bucket(bucket_index).unwrap_or(0) as usize;
        while index != 0 {
            if index >= table.symbol_count {
                return Err(LoadError::UnsupportedHeader);
            }
            let symbol = read_symbol_entry(table.symbols, index)?;
            if symbol.shndx != SHN_UNDEF
                && (symbol.binding == STB_GLOBAL || symbol.binding == STB_WEAK)
            {
                let name = symbol_name_bytes(table.strings, symbol.name_offset)?;
                namespace.insert(
                    name,
                    symbol.binding == STB_WEAK,
                    load_base.wrapping_add(symbol.value),
                )?;
            }
            index = hash.chain(index) as usize;
        }
    }
    Ok(())
}

/// Apply a raw relocation table containing `R_X86_64_RELATIVE`,
/// `R_X86_64_GLOB_DAT`, and `R_X86_64_JUMP_SLOT` entries against the load's
/// shared symbol namespace. Undefined strong symbols that cannot be resolved
/// fail with [`LoadError::UnresolvedSymbol`]; undefined weak symbols resolve
/// to 0. Symbol-relative types require the consumer module's own dynsym to
/// decode `r_sym`, supplied via `consumer`.
fn apply_dynamic_relocations(
    rela_table: &[u8],
    load_base: u64,
    consumer: Option<&ElfSymbolTable>,
    namespace: &SymbolNamespace,
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
        let reloc_type = r_info & 0xffff_ffff;
        let symbol_index = (r_info >> 32) as usize;
        match reloc_type {
            ELF_RELOC_RELATIVE => {
                write_word(
                    r_offset.wrapping_add(load_base),
                    load_base.wrapping_add(addend),
                )?;
            }
            ELF_RELOC_GLOB_DAT | ELF_RELOC_JUMP_SLOT => {
                let table = consumer.ok_or(LoadError::UnsupportedRelocation)?;
                if symbol_index >= table.symbol_count {
                    return Err(LoadError::UnsupportedRelocation);
                }
                let symbol = read_symbol_entry(table.symbols, symbol_index)?;
                let name = symbol_name_bytes(table.strings, symbol.name_offset)?;
                let value = if symbol.shndx != SHN_UNDEF {
                    // Defined in this module: bias its link-time value.
                    load_base.wrapping_add(symbol.value)
                } else if let Some(address) = namespace.lookup(name) {
                    address
                } else if symbol.binding == STB_WEAK {
                    0
                } else {
                    let mut resolved = [0u8; 32];
                    let len = name.len().min(resolved.len());
                    resolved[..len].copy_from_slice(&name[..len]);
                    return Err(LoadError::UnresolvedSymbol {
                        name: resolved,
                        len: len as u8,
                    });
                };
                write_word(r_offset.wrapping_add(load_base), value)?;
            }
            _ => return Err(LoadError::UnsupportedRelocation),
        }
    }
    Ok(entry_count)
}

/// One dependency image validated as an ELF64 shared object, ready to map.
struct ElfDependencyPlan {
    loads: [ElfLoadSegment; MAX_ELF_LOAD_SEGMENTS],
    load_count: usize,
    dynamic_range: Option<(usize, usize)>,
    /// Footprint from the lowest segment start to the highest segment end;
    /// the deterministic companion base must contain it.
    span: u64,
}

/// Validate resolved dependency bytes as an `ET_DYN` ELF64 object for the
/// target machine and collect its PT_LOAD/PT_DYNAMIC program headers. Any
/// validation failure surfaces as [`LoadError::DependencyInvalid`].
fn plan_elf_dependency(
    bytes: &[u8],
    expected_machine: ElfMachine,
) -> Result<ElfDependencyPlan, LoadError> {
    plan_elf_dependency_inner(bytes, expected_machine).map_err(|error| match error {
        error @ (LoadError::FrameExhausted | LoadError::Mapping(_)) => error,
        _ => LoadError::DependencyInvalid,
    })
}

fn plan_elf_dependency_inner(
    bytes: &[u8],
    expected_machine: ElfMachine,
) -> Result<ElfDependencyPlan, LoadError> {
    if bytes.len() < 64 {
        return Err(LoadError::Truncated);
    }
    if bytes[4] != ELF_CLASS_64 || bytes[5] != ELF_DATA_LSB || bytes[6] != ELF_VERSION_CURRENT {
        return Err(LoadError::UnsupportedAbi);
    }
    let elf_type = read_u16_le(bytes, 16)?;
    // Companions must be position-independent shared objects.
    if elf_type != ELF_TYPE_DYN {
        return Err(LoadError::UnsupportedAbi);
    }
    if read_u16_le(bytes, 18)? != expected_machine.as_u16() {
        return Err(LoadError::UnsupportedMachine);
    }
    let phoff = read_u64_le(bytes, 32)? as usize;
    let phentsize = read_u16_le(bytes, 54)? as usize;
    let phnum = read_u16_le(bytes, 56)? as usize;
    if phentsize != ELF_PROGRAM_HEADER_LEN || phnum == 0 {
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
    for index in 0..phnum {
        let header = phoff
            .checked_add(index * phentsize)
            .ok_or(LoadError::Truncated)?;
        let program = bytes
            .get(header..header + phentsize)
            .ok_or(LoadError::Truncated)?;
        match read_u32_le(program, 0)? {
            ELF_SEGMENT_DYNAMIC => {
                if dynamic_range.is_some() {
                    return Err(LoadError::UnsupportedHeader);
                }
                let dyn_offset = read_u64_le(program, 8)? as usize;
                let dyn_size = read_u64_le(program, 32)? as usize;
                bytes
                    .get(
                        dyn_offset
                            ..dyn_offset
                                .checked_add(dyn_size)
                                .ok_or(LoadError::Truncated)?,
                    )
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
    let mut span_low = u64::MAX;
    let mut span_high = 0u64;
    for segment in &loads[..load_count] {
        if segment.virtual_address % PAGE_SIZE_BYTES != 0
            || segment.file_size > segment.memory_size
            || bytes
                .get(segment.file_offset..segment.file_offset + segment.file_size)
                .is_none()
        {
            return Err(LoadError::UnsupportedHeader);
        }
        span_low = span_low.min(segment.virtual_address);
        span_high = span_high.max(
            segment
                .virtual_address
                .saturating_add(segment.memory_size as u64),
        );
    }
    Ok(ElfDependencyPlan {
        loads,
        load_count,
        dynamic_range,
        span: span_high.saturating_sub(span_low),
    })
}

/// An ELF64 companion mapped into the address space, awaiting the shared
/// relocation phase that runs once every image is present.
struct PendingElfModule {
    image: &'static [u8],
    loads: [ElfLoadSegment; MAX_ELF_LOAD_SEGMENTS],
    load_count: usize,
    mapped: [MappedLoadSegment; MAX_ELF_LOAD_SEGMENTS],
    mapped_count: usize,
    load_base: u64,
    info: ElfDynamicInfo,
    tables: Option<ElfSymbolTable<'static>>,
}

impl PendingElfModule {
    fn apply_relocations(&self, namespace: &SymbolNamespace) -> Result<(), LoadError> {
        for relocation_table in [self.info.rela, self.info.jmprel].into_iter().flatten() {
            let (table_vaddr, table_size) = relocation_table;
            let bytes = locate_file_range(
                self.image,
                &self.loads[..self.load_count],
                table_vaddr,
                table_size as usize,
            )
            .ok_or(LoadError::UnsupportedRelocation)?;
            apply_dynamic_relocations(
                bytes,
                self.load_base,
                self.tables.as_ref(),
                namespace,
                |target, value| write_mapped_word(&self.mapped, self.mapped_count, target, value),
            )?;
        }
        Ok(())
    }
}

/// Map an already-planned ELF companion's segments at `base`, register its
/// exports into `namespace`, and return its pending relocation state.
fn map_elf_dependency(
    bytes: &'static [u8],
    plan: &ElfDependencyPlan,
    base: VirtualAddress,
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
    namespace: &mut SymbolNamespace,
) -> Result<PendingElfModule, LoadError> {
    let page_size = PAGE_SIZE_BYTES as usize;
    let mut mapped = [MappedLoadSegment {
        virtual_start: 0,
        memory_size: 0,
        frame_base: PhysicalAddress::new(0),
    }; MAX_ELF_LOAD_SEGMENTS];
    let mut mapped_count = 0usize;
    for segment in &plan.loads[..plan.load_count] {
        let virtual_start = base
            .as_u64()
            .checked_add(segment.virtual_address)
            .ok_or(LoadError::DependencyInvalid)?;
        let payload = bytes
            .get(segment.file_offset..segment.file_offset + segment.file_size)
            .ok_or(LoadError::DependencyInvalid)?;
        let segment_pages = segment.memory_size.div_ceil(page_size);
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
            mapped[mapped_count] = MappedLoadSegment {
                virtual_start,
                memory_size: (pages * page_size) as u64,
                frame_base,
            };
            mapped_count += 1;
        }
    }

    let load_base = base.as_u64();
    let (info, tables) = match plan.dynamic_range {
        Some((dyn_offset, dyn_size)) => {
            let info = parse_dynamic_info(&bytes[dyn_offset..dyn_offset + dyn_size])
                .map_err(|_| LoadError::DependencyInvalid)?;
            let tables = resolve_symbol_tables(bytes, &plan.loads[..plan.load_count], &info)
                .map_err(|_| LoadError::DependencyInvalid)?;
            (info, tables)
        }
        None => (ElfDynamicInfo::default(), None),
    };
    if let Some(table) = &tables {
        register_module_exports(table, load_base, namespace)?;
    }
    Ok(PendingElfModule {
        image: bytes,
        loads: plan.loads,
        load_count: plan.load_count,
        mapped,
        mapped_count,
        load_base,
        info,
        tables,
    })
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
    if !image_window_contains(
        stack_bottom_value,
        (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES,
    ) {
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
                image
                    .get(
                        dyn_offset
                            ..dyn_offset
                                .checked_add(dyn_size)
                                .ok_or(LoadError::Truncated)?,
                    )
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
            .map(|segment| {
                segment
                    .virtual_address
                    .saturating_add(segment.memory_size as u64)
            })
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
            load_base
                .checked_add(segment.virtual_address)
                .ok_or(LoadError::UnsupportedHeader)?
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

    // Process PT_DYNAMIC: register this image's exports, then apply
    // `R_X86_64_RELATIVE` fixups and resolve `R_X86_64_GLOB_DAT` /
    // `R_X86_64_JUMP_SLOT` entries against the load's symbol namespace.
    // Position-independent images are only executable once these absolute
    // references have been fixed; every write stays inside mapped segments.
    if let Some((dyn_offset, dyn_size)) = dynamic_range {
        let mut namespace = Box::new(SymbolNamespace::EMPTY);
        let dynamic_bytes = &image[dyn_offset..dyn_offset + dyn_size];
        let info = parse_dynamic_info(dynamic_bytes)?;
        let tables = resolve_symbol_tables(image, &loads[..load_count], &info)?;
        if let Some(table) = &tables {
            register_module_exports(table, load_base, namespace.as_mut())?;
        }
        for (table_vaddr, table_size) in [info.rela, info.jmprel].into_iter().flatten() {
            let bytes = locate_file_range(
                image,
                &loads[..load_count],
                table_vaddr,
                table_size as usize,
            )
            .ok_or(LoadError::UnsupportedRelocation)?;
            apply_dynamic_relocations(
                bytes,
                load_base,
                tables.as_ref(),
                &namespace,
                |target, value| write_mapped_word(&mapped_segments, load_count, target, value),
            )?;
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
        image_base: VirtualAddress::new(if position_independent {
            load_base
        } else {
            image_base
        }),
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
        headers.extend(program_header(
            ELF_SEGMENT_LOAD,
            ELF_FLAG_EXECUTE,
            0x1000,
            0x1000,
            8,
            0x2000,
        ));
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

    /// Specification for a synthetic ELF64 shared-object fixture exercising
    /// the dynamic-symbol/relocation paths on the host.
    struct SharedSpec<'a> {
        /// (name, link-time st_value) registered with STB_LOCAL binding.
        locals: &'a [(&'a [u8], u64)],
        /// (name, weak, link-time st_value) exported definitions.
        exports: &'a [(&'a [u8], bool, u64)],
        /// (name, weak) undefined imports resolved against other modules.
        imports: &'a [(&'a [u8], bool)],
        /// (r_offset, reloc type, dynsym index) rows for `.rela.dyn`.
        rela_dyn: &'a [(u64, u64, u16)],
        /// (r_offset, reloc type, dynsym index) rows for `.rela.plt`.
        rela_plt: &'a [(u64, u64, u16)],
        /// DT_HASH bucket count (1 forces chain walks through collisions).
        hash_buckets: usize,
    }

    struct SharedFixture {
        image: Vec<u8>,
        loads: [ElfLoadSegment; 1],
    }

    fn build_shared_object(spec: SharedSpec) -> SharedFixture {
        const SEG_FILE_OFFSET: u64 = 0x1000;
        const SEG_VADDR: u64 = 0x3000;

        let export_count = spec.exports.len();
        let import_count = spec.imports.len();
        let symbol_count = 1 + export_count + import_count + spec.locals.len();

        // Dynsym order: null slot, exports, imports, locals.
        let mut names: Vec<&[u8]> = vec![b""];
        for (name, _, _) in spec.exports {
            names.push(name);
        }
        for (name, _) in spec.imports {
            names.push(name);
        }
        for (name, _) in spec.locals {
            names.push(name);
        }

        let mut strings = vec![0u8];
        let mut name_offsets = vec![0u32];
        for name in &names[1..] {
            name_offsets.push(strings.len() as u32);
            strings.extend_from_slice(name);
            strings.push(0);
        }

        let mut symbols = Vec::new();
        {
            let mut push_sym = |name_off: u32, bind: u8, shndx: u16, value: u64| {
                push_u32(&mut symbols, name_off);
                symbols.push(bind << 4);
                symbols.push(0);
                push_u16(&mut symbols, shndx);
                push_u64(&mut symbols, value);
                push_u64(&mut symbols, 0);
            };
            push_sym(0, 0, 0, 0);
            for (index, (_, weak, value)) in spec.exports.iter().enumerate() {
                push_sym(
                    name_offsets[index + 1],
                    if *weak { STB_WEAK } else { STB_GLOBAL },
                    1,
                    *value,
                );
            }
            for (index, (_, weak)) in spec.imports.iter().enumerate() {
                push_sym(
                    name_offsets[1 + export_count + index],
                    if *weak { STB_WEAK } else { STB_GLOBAL },
                    0,
                    0,
                );
            }
            for (index, (_, value)) in spec.locals.iter().enumerate() {
                push_sym(
                    name_offsets[1 + export_count + import_count + index],
                    0,
                    1,
                    *value,
                );
            }
        }

        let nbucket = spec.hash_buckets.max(1);
        let mut buckets = vec![0u32; nbucket];
        let mut chains = vec![0u32; symbol_count];
        for index in 1..symbol_count {
            let slot = (elf_hash(names[index]) as usize) % nbucket;
            chains[index] = buckets[slot];
            buckets[slot] = index as u32;
        }
        let mut hash_bytes = Vec::new();
        push_u32(&mut hash_bytes, nbucket as u32);
        push_u32(&mut hash_bytes, symbol_count as u32);
        for bucket in buckets {
            push_u32(&mut hash_bytes, bucket);
        }
        for chain in chains {
            push_u32(&mut hash_bytes, chain);
        }

        let slot_count = spec.rela_dyn.len() + spec.rela_plt.len();
        let has_dyn = !spec.rela_dyn.is_empty();
        let has_plt = !spec.rela_plt.is_empty();
        let dyn_entry_count = 5usize + usize::from(has_dyn) * 2 + usize::from(has_plt) * 2;
        let dyn_len = (dyn_entry_count * 16) as u64;
        let mut lengths: Vec<u64> = vec![
            dyn_len,
            strings.len() as u64,
            symbols.len() as u64,
            hash_bytes.len() as u64,
        ];
        if has_dyn {
            lengths.push((spec.rela_dyn.len() * 24) as u64);
        }
        if has_plt {
            lengths.push((spec.rela_plt.len() * 24) as u64);
        }
        lengths.push((slot_count * 8) as u64);

        let mut file_cursor = SEG_FILE_OFFSET;
        let mut offsets: Vec<u64> = Vec::new();
        for length in &lengths {
            offsets.push(file_cursor);
            file_cursor += length;
        }
        let payload_len = (file_cursor - SEG_FILE_OFFSET) as usize;
        let seg_memory_size =
            payload_len.div_ceil(PAGE_SIZE_BYTES as usize) * PAGE_SIZE_BYTES as usize;
        let vaddr_of = |entry: usize| SEG_VADDR + (offsets[entry] - SEG_FILE_OFFSET);

        let mut dynamic = Vec::new();
        {
            let mut push_entry = |tag: u64, value: u64| {
                push_u64(&mut dynamic, tag);
                push_u64(&mut dynamic, value);
            };
            push_entry(DYNAMIC_TAG_SYMTAB, vaddr_of(2));
            push_entry(DYNAMIC_TAG_STRTAB, vaddr_of(1));
            push_entry(DYNAMIC_TAG_STRSZ, strings.len() as u64);
            push_entry(DYNAMIC_TAG_HASH, vaddr_of(3));
            if has_dyn {
                push_entry(DYNAMIC_TAG_RELA, vaddr_of(4));
                push_entry(DYNAMIC_TAG_RELASZ, lengths[4]);
            }
            if has_plt {
                let plt_entry = usize::from(has_dyn) + 4;
                push_entry(DYNAMIC_TAG_JMPREL, vaddr_of(plt_entry));
                push_entry(DYNAMIC_TAG_PLTRELSZ, lengths[plt_entry]);
            }
            push_entry(DYNAMIC_TAG_NULL, 0);
        }
        assert_eq!(dynamic.len() as u64, dyn_len);

        let mut rela_blob = Vec::new();
        for &(offset, kind, index) in spec.rela_dyn {
            push_u64(&mut rela_blob, offset);
            push_u64(&mut rela_blob, kind | ((index as u64) << 32));
            push_u64(&mut rela_blob, 0);
        }
        let mut plt_blob = Vec::new();
        for &(offset, kind, index) in spec.rela_plt {
            push_u64(&mut plt_blob, offset);
            push_u64(&mut plt_blob, kind | ((index as u64) << 32));
            push_u64(&mut plt_blob, 0);
        }
        let mut image = Vec::new();
        image.extend_from_slice(&ELF_MAGIC);
        image.push(ELF_CLASS_64);
        image.push(ELF_DATA_LSB);
        image.push(ELF_VERSION_CURRENT);
        image.extend_from_slice(&[0; 9]);
        push_u16(&mut image, ELF_TYPE_DYN);
        push_u16(&mut image, ELF_MACHINE_X86_64);
        push_u32(&mut image, 1); // e_version
        push_u64(&mut image, 0x1040); // e_entry (base-relative; unused by these paths)
        push_u64(&mut image, 64); // e_phoff
        push_u64(&mut image, 0); // e_shoff
        push_u32(&mut image, 0); // e_flags
        push_u16(&mut image, 64); // e_ehsize
        push_u16(&mut image, ELF_PROGRAM_HEADER_LEN as u16);
        push_u16(&mut image, 2); // e_phnum
        push_u16(&mut image, 0);
        push_u16(&mut image, 0);
        push_u16(&mut image, 0);
        image.extend(program_header(
            ELF_SEGMENT_LOAD,
            ELF_FLAG_WRITE,
            SEG_FILE_OFFSET,
            SEG_VADDR,
            payload_len as u64,
            seg_memory_size as u64,
        ));
        image.extend(program_header(
            ELF_SEGMENT_DYNAMIC,
            ELF_FLAG_WRITE,
            offsets[0],
            vaddr_of(0),
            dyn_len,
            dyn_len,
        ));
        while image.len() < SEG_FILE_OFFSET as usize {
            image.push(0);
        }
        image.extend_from_slice(&dynamic);
        image.extend_from_slice(&strings);
        image.extend_from_slice(&symbols);
        image.extend_from_slice(&hash_bytes);
        image.extend_from_slice(&rela_blob);
        image.extend_from_slice(&plt_blob);
        image.resize(file_cursor as usize, 0);

        SharedFixture {
            image,
            loads: [ElfLoadSegment {
                flags: ELF_FLAG_WRITE,
                file_offset: SEG_FILE_OFFSET as usize,
                virtual_address: SEG_VADDR,
                file_size: payload_len,
                memory_size: seg_memory_size,
            }],
        }
    }

    /// Byte-buffer stand-in for one module's mapped image at `load_base`.
    fn module_memory(size: usize) -> Vec<u8> {
        vec![0u8; size]
    }

    fn write_into(
        memory: &mut [u8],
        load_base: u64,
        target: u64,
        value: u64,
    ) -> Result<(), LoadError> {
        let index = target
            .checked_sub(load_base)
            .ok_or(LoadError::UnsupportedRelocation)? as usize;
        let end = index
            .checked_add(8)
            .ok_or(LoadError::UnsupportedRelocation)?;
        if end > memory.len() {
            return Err(LoadError::UnsupportedRelocation);
        }
        memory[index..end].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn read_word(memory: &[u8], load_base: u64, vaddr: u64) -> u64 {
        let index = (vaddr - load_base) as usize;
        u64::from_le_bytes(memory[index..index + 8].try_into().unwrap())
    }

    /// Resolve a fixture's dynamic info exactly like the loader does. The
    /// fixture bytes are leaked so table borrows become `'static`, matching
    /// the boot-store-backed images the real loader resolves.
    fn fixture_tables(fixture: &SharedFixture) -> ElfDynamicInfoPlusTables {
        let loads = &fixture.loads[..];
        let phoff = u64::from_le_bytes(fixture.image[32..40].try_into().unwrap()) as usize;
        // PT_DYNAMIC is the second program header in every fixture.
        let header = &fixture.image[phoff + 56..phoff + 112];
        let dyn_offset = u64::from_le_bytes(header[8..16].try_into().unwrap()) as usize;
        let dyn_size = u64::from_le_bytes(header[32..40].try_into().unwrap()) as usize;
        let static_image: &'static [u8] = Box::leak(fixture.image.clone().into_boxed_slice());
        let info = parse_dynamic_info(&static_image[dyn_offset..dyn_offset + dyn_size])
            .expect("dynamic info parses");
        let tables = resolve_symbol_tables(static_image, loads, &info).expect("tables locate");
        (info, tables)
    }

    type ElfDynamicInfoPlusTables = (ElfDynamicInfo, Option<ElfSymbolTable<'static>>);

    #[test]
    fn relative_relocations_apply_into_byte_buffer() {
        let image = golden_minimal_et_dyn();
        assert_eq!(image.len(), 0x2248);
        let loads = [
            ElfLoadSegment {
                flags: ELF_FLAG_EXECUTE,
                file_offset: 0x1000,
                virtual_address: 0x1000,
                file_size: 8,
                memory_size: 0x2000,
            },
            ElfLoadSegment {
                flags: ELF_FLAG_WRITE,
                file_offset: 0x2000,
                virtual_address: 0x4000,
                file_size: 0x300,
                memory_size: 0x1000,
            },
        ];
        let dynamic_bytes = &image[0x2100..0x2100 + 48];
        let info = parse_dynamic_info(dynamic_bytes).expect("dynamic info parses");
        let (rela_vaddr, rela_size) = info.rela.expect("rela table present");
        assert_eq!(rela_vaddr, 0x4200);
        assert_eq!(rela_size, 72);
        assert_eq!(info.jmprel, None);

        let load_base = choose_pie_load_base(0x5000, TEST_STACK_TOP).expect("base chosen");
        assert_eq!(load_base, USER_IMAGE_WINDOW_START);
        assert_eq!(load_base % PAGE_SIZE_BYTES, 0);

        let rela_bytes =
            locate_file_range(image.as_slice(), &loads, rela_vaddr, rela_size as usize)
                .expect("rela table resolves into the file");

        // Byte-buffer stand-in for the mapped image [load_base, load_base+0x5000).
        let mut memory = vec![0u8; 0x5000];
        let applied = apply_dynamic_relocations(
            rela_bytes,
            load_base,
            None,
            &SymbolNamespace::EMPTY,
            |target, value| write_into(&mut memory, load_base, target, value),
        )
        .expect("all relocations apply");
        assert_eq!(applied, 3);

        assert_eq!(
            read_word(&memory, load_base, load_base + 0x4100),
            load_base + 0x1234
        );
        assert_eq!(
            read_word(&memory, load_base, load_base + 0x4110),
            (load_base as i64 - 8) as u64
        );
        assert_eq!(read_word(&memory, load_base, load_base + 0x4140), load_base);
    }

    #[test]
    fn unsupported_relocation_types_are_rejected() {
        let mut table = Vec::new();
        push_u64(&mut table, 0x4100);
        push_u64(&mut table, 1); // R_X86_64_64 — not supported
        push_u64(&mut table, 0);
        let mut writes = 0usize;
        let result = apply_dynamic_relocations(
            &table,
            TEST_BASE,
            None,
            &SymbolNamespace::EMPTY,
            |_target, _value| {
                writes += 1;
                Ok(())
            },
        );
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
        let mut memory = module_memory(0x1000);
        let result = apply_dynamic_relocations(
            &table,
            TEST_BASE,
            None,
            &SymbolNamespace::EMPTY,
            |target, value| write_into(&mut memory, TEST_BASE, target, value),
        );
        assert_eq!(result, Err(LoadError::UnsupportedRelocation));
    }

    #[test]
    fn malformed_dynamic_blocks_are_rejected() {
        assert_eq!(parse_dynamic_info(&[]), Ok(ElfDynamicInfo::default()));

        // DT_RELA without DT_RELASZ is malformed.
        let mut dynamic = Vec::new();
        push_u64(&mut dynamic, DYNAMIC_TAG_RELA);
        push_u64(&mut dynamic, 0x4300);
        push_u64(&mut dynamic, DYNAMIC_TAG_NULL);
        push_u64(&mut dynamic, 0);
        assert_eq!(
            parse_dynamic_info(&dynamic),
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
            parse_dynamic_info(&dynamic),
            Err(LoadError::UnsupportedHeader)
        );

        // Entries after DT_NULL are ignored.
        let mut dynamic = Vec::new();
        push_u64(&mut dynamic, DYNAMIC_TAG_NULL);
        push_u64(&mut dynamic, 0);
        push_u64(&mut dynamic, DYNAMIC_TAG_RELASZ);
        push_u64(&mut dynamic, 999);
        assert_eq!(parse_dynamic_info(&dynamic), Ok(ElfDynamicInfo::default()));
    }

    #[test]
    fn pie_load_base_is_deterministic_and_page_aligned() {
        let stack_bottom = TEST_STACK_TOP - (USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES;
        assert_eq!(
            choose_pie_load_base(0x5000, stack_bottom),
            Ok(USER_IMAGE_WINDOW_START)
        );
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
            apply_dynamic_relocations(&table, TEST_BASE, None, &SymbolNamespace::EMPTY, |_, _| Ok(
                ()
            )),
            Err(LoadError::UnsupportedHeader)
        );
        table.resize(24, 0);
        // Single RELATIVE entry with zero offset/addend applies cleanly.
        table[8] = 8; // r_info: R_X86_64_RELATIVE
        assert_eq!(
            apply_dynamic_relocations(&table, TEST_BASE, None, &SymbolNamespace::EMPTY, |_, _| Ok(
                ()
            )),
            Ok(1)
        );
    }

    #[test]
    fn sysv_hash_lookup_walks_buckets_and_chains() {
        let provider = build_shared_object(SharedSpec {
            locals: &[],
            exports: &[
                (b"shared_value", false, 0x1234),
                (b"other_fn", false, 0x2040),
            ],
            imports: &[],
            rela_dyn: &[],
            rela_plt: &[],
            hash_buckets: 1, // every symbol collides into one bucket chain
        });
        let (_, tables) = fixture_tables(&provider);
        let table = tables.expect("provider declares a complete symbol interface");
        assert_eq!(table.lookup(b"shared_value"), Some(0x1234));
        assert_eq!(table.lookup(b"other_fn"), Some(0x2040));
        assert_eq!(table.lookup(b"missing"), None);
        assert_eq!(table.lookup(b""), None);
        assert_eq!(elf_hash(b"shared_value"), elf_hash(b"shared_value"));
    }

    #[test]
    fn export_registration_skips_locals_and_undefs_and_biases_by_base() {
        let module = build_shared_object(SharedSpec {
            locals: &[(b"hidden_local", 0x10)],
            exports: &[(b"global_fn", false, 0x40), (b"weak_var", true, 0x80)],
            imports: &[],
            rela_dyn: &[],
            rela_plt: &[],
            hash_buckets: 2,
        });
        let (_, tables) = fixture_tables(&module);
        let table = tables.expect("symbol interface present");
        let load_base = TEST_BASE + 0x20_0000;
        let mut namespace = SymbolNamespace::EMPTY;
        register_module_exports(&table, load_base, &mut namespace).expect("exports register");
        assert_eq!(namespace.count, 2);
        assert_eq!(namespace.lookup(b"global_fn"), Some(load_base + 0x40));
        assert_eq!(namespace.lookup(b"weak_var"), Some(load_base + 0x80));
        assert_eq!(namespace.lookup(b"hidden_local"), None);
    }

    #[test]
    fn namespace_override_order_main_last_wins() {
        let mut namespace = SymbolNamespace::EMPTY;
        namespace.insert(b"foo", false, 0xAA).unwrap(); // dep A
        namespace.insert(b"foo", false, 0xBB).unwrap(); // dep B
        assert_eq!(namespace.lookup(b"foo"), Some(0xBB));
        namespace.insert(b"foo", false, 0xCC).unwrap(); // main registers last
        assert_eq!(namespace.lookup(b"foo"), Some(0xCC));

        // Strong beats weak regardless of registration order.
        namespace.insert(b"bar", true, 0x11).unwrap();
        namespace.insert(b"bar", false, 0x22).unwrap();
        assert_eq!(namespace.lookup(b"bar"), Some(0x22));
        namespace.insert(b"baz", false, 0x33).unwrap();
        namespace.insert(b"baz", true, 0x44).unwrap();
        assert_eq!(namespace.lookup(b"baz"), Some(0x33));
    }

    #[test]
    fn glob_dat_jump_slot_and_weak_zero_resolve_across_modules() {
        let provider_base = TEST_BASE;
        let consumer_base = TEST_BASE + 0x40_0000;

        let provider = build_shared_object(SharedSpec {
            locals: &[],
            exports: &[(b"shared_value", false, 0x1234)],
            imports: &[],
            rela_dyn: &[],
            rela_plt: &[],
            hash_buckets: 1,
        });
        let (_, provider_tables) = fixture_tables(&provider);
        let mut namespace = SymbolNamespace::EMPTY;
        register_module_exports(
            provider_tables.as_ref().expect("provider exports"),
            provider_base,
            &mut namespace,
        )
        .expect("provider exports register");

        // Consumer: GLOB_DAT on a strong import defined elsewhere, GLOB_DAT on
        // an absent WEAK symbol (resolves to 0), one RELATIVE fixup, and a
        // JUMP_SLOT routed through DT_JMPREL back to the strong symbol.
        let consumer = build_shared_object(SharedSpec {
            locals: &[],
            exports: &[],
            imports: &[(b"shared_value", false), (b"absent_weak", true)],
            rela_dyn: &[
                (0x30, ELF_RELOC_GLOB_DAT, 1),
                (0x38, ELF_RELOC_GLOB_DAT, 2),
                (0x40, ELF_RELOC_RELATIVE, 0),
            ],
            rela_plt: &[(0x48, ELF_RELOC_JUMP_SLOT, 1)],
            hash_buckets: 2,
        });
        let (info, consumer_tables) = fixture_tables(&consumer);
        let consumer_table = consumer_tables.expect("consumer dynsym present");
        let mut memory = module_memory(PAGE_SIZE_BYTES as usize);
        let mut applied_total = 0usize;
        for (vaddr, size) in [info.rela, info.jmprel].into_iter().flatten() {
            let bytes: &'static [u8] = Box::leak(consumer.image.clone().into_boxed_slice());
            let bytes = locate_file_range(bytes, &consumer.loads[..], vaddr, size as usize)
                .expect("relocation table locates");
            applied_total += apply_dynamic_relocations(
                bytes,
                consumer_base,
                Some(&consumer_table),
                &namespace,
                |target, value| write_into(&mut memory, consumer_base, target, value),
            )
            .expect("consumer relocations resolve");
        }
        assert_eq!(applied_total, 4);
        assert_eq!(
            read_word(&memory, consumer_base, consumer_base + 0x30),
            provider_base + 0x1234
        );
        assert_eq!(read_word(&memory, consumer_base, consumer_base + 0x38), 0);
        // RELATIVE row stores addend 0 in this fixture: word becomes base+0.
        assert_eq!(
            read_word(&memory, consumer_base, consumer_base + 0x40),
            consumer_base
        );
        assert_eq!(
            read_word(&memory, consumer_base, consumer_base + 0x48),
            provider_base + 0x1234
        );
    }

    #[test]
    fn unresolved_strong_symbol_fails_with_name() {
        let provider = build_shared_object(SharedSpec {
            locals: &[],
            exports: &[(b"unrelated", false, 0x8)],
            imports: &[],
            rela_dyn: &[],
            rela_plt: &[],
            hash_buckets: 1,
        });
        let (_, provider_tables) = fixture_tables(&provider);
        let mut namespace = SymbolNamespace::EMPTY;
        register_module_exports(
            provider_tables.as_ref().expect("provider exports"),
            TEST_BASE,
            &mut namespace,
        )
        .expect("provider exports register");

        let consumer = build_shared_object(SharedSpec {
            locals: &[],
            exports: &[],
            imports: &[(b"strong_missing", false)],
            rela_dyn: &[],
            rela_plt: &[(0x30, ELF_RELOC_JUMP_SLOT, 1)],
            hash_buckets: 1,
        });
        let (info, consumer_tables) = fixture_tables(&consumer);
        let consumer_table = consumer_tables.expect("consumer dynsym present");
        let mut memory = module_memory(PAGE_SIZE_BYTES as usize);
        let (vaddr, size) = info.jmprel.expect("jmprel present");
        let bytes: &'static [u8] = Box::leak(consumer.image.clone().into_boxed_slice());
        let bytes = locate_file_range(bytes, &consumer.loads[..], vaddr, size as usize)
            .expect("plt locates");
        let error = apply_dynamic_relocations(
            bytes,
            TEST_BASE + 0x40_0000,
            Some(&consumer_table),
            &namespace,
            |target, value| write_into(&mut memory, TEST_BASE + 0x40_0000, target, value),
        )
        .expect_err("strong undefined symbol must fail the load");
        match error {
            LoadError::UnresolvedSymbol { name, len } => {
                assert_eq!(&name[..len as usize], b"strong_missing");
            }
            other => panic!("expected UnresolvedSymbol, got {other:?}"),
        }
    }

    #[test]
    fn symbol_types_require_a_consumer_dynsym() {
        let mut table = Vec::new();
        push_u64(&mut table, 0x30);
        push_u64(&mut table, ELF_RELOC_GLOB_DAT | (1u64 << 32));
        push_u64(&mut table, 0);
        let result =
            apply_dynamic_relocations(&table, TEST_BASE, None, &SymbolNamespace::EMPTY, |_, _| {
                Ok(())
            });
        assert_eq!(result, Err(LoadError::UnsupportedRelocation));
    }
}
