//! PE/COFF detection and classification groundwork for the Windows-runtime
//! roadmap (S11).
//!
//! SCOPE — honest status: this module DETECTS and CLASSIFIES Windows PE
//! images on the runtime-service exec path. It does NOT execute them. The
//! real mountain is a WinAPI/WinNT ABI layer (peb/teb, syscall thunks,
//! loader semantics); until that exists, every detected PE image is refused
//! with `RuntimeStatus::Unsupported`. Parsing mirrors the kernel loader's
//! ELF gate style: pure, host-testable, no allocation, `core` only.

/// `MZ` DOS header magic.
const DOS_MAGIC: [u8; 2] = *b"MZ";
/// `PE\0\0` signature that follows the DOS stub at `e_lfanew`.
const PE_SIGNATURE: [u8; 4] = [0x50, 0x00, 0x00, 0x00];
/// Offset of `e_lfanew` inside the 64-byte DOS header.
const E_LFANEW_OFFSET: usize = 0x3c;
const COFF_HEADER_LEN: usize = 20;
const SECTION_HEADER_LEN: usize = 40;
/// Sections actually extracted per image; the COFF count can reach 65535 so
/// extraction is capped and the remainder reported via `section_count`.
pub(crate) const MAX_PARSED_SECTIONS: usize = 16;

const MACHINE_I386: u16 = 0x14c;
pub(crate) const MACHINE_X86_64: u16 = 0x8664;
const MACHINE_AARCH64: u16 = 0xaa64;
/// Optional-header magic for PE32+ (64-bit).
pub(crate) const OPT_MAGIC_PE32PLUS: u16 = 0x20b;
/// Optional-header magic for classic PE32.
pub(crate) const OPT_MAGIC_PE32: u16 = 0x10b;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeError {
    /// Fewer bytes than any required header window.
    Truncated,
    /// `MZ` present but `e_lfanew` out of range or signature not `PE\0\0`.
    BadSignature,
    /// COFF declares an optional header but it does not fit in the probe.
    MissingOptionalHeader,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CoffHeader {
    pub(crate) machine: u16,
    pub(crate) section_count: u16,
    pub(crate) timestamp: u32,
    pub(crate) symbol_table_ptr: u32,
    pub(crate) symbol_count: u32,
    pub(crate) optional_header_size: u16,
    pub(crate) characteristics: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeSectionEntry {
    pub(crate) name: [u8; 8],
    pub(crate) virtual_size: u32,
    pub(crate) virtual_address: u32,
    pub(crate) raw_size: u32,
    pub(crate) raw_ptr: u32,
    pub(crate) characteristics: u32,
}

impl PeSectionEntry {
    /// NUL-trimmed ASCII name, or `None` when the field holds non-ASCII
    /// bytes (long-name `/nnnnn` string-table references included).
    pub(crate) fn name_str(&self) -> Option<&str> {
        let end = self.name.iter().position(|byte| *byte == 0).unwrap_or(8);
        core::str::from_utf8(&self.name[..end]).ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeImage {
    pub(crate) coff: CoffHeader,
    pub(crate) optional_magic: u16,
    pub(crate) entry_rva: u32,
    pub(crate) image_base: u64,
    pub(crate) subsystem: u16,
    /// Extraction-capped section table (`sections_len <= section_count`).
    pub(crate) sections: [PeSectionEntry; MAX_PARSED_SECTIONS],
    pub(crate) sections_len: usize,
}

impl PeImage {
    /// Extracted section entries (never exceeds `MAX_PARSED_SECTIONS`).
    pub(crate) fn section_slice(&self) -> &[PeSectionEntry] {
        &self.sections[..self.sections_len]
    }

    /// Classify against what the future Windows runtime would target:
    /// x86_64 PE32+. Everything else is rejected with its reason.
    pub(crate) fn classify(&self) -> PeClass {
        match self.coff.machine {
            MACHINE_X86_64 => {}
            other => return PeClass::UnsupportedMachine { machine: other },
        }
        match self.optional_magic {
            OPT_MAGIC_PE32PLUS => PeClass::Pe32PlusX64,
            OPT_MAGIC_PE32 => PeClass::Pe32,
            other => PeClass::UnknownOptionalHeader { magic: other },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeClass {
    /// The shape the Windows runtime roadmap targets. Detection only —
    /// execution requires a WinAPI ABI layer that does not exist yet.
    Pe32PlusX64,
    /// Valid PE, wrong architecture.
    UnsupportedMachine {
        machine: u16,
    },
    /// x86_64 but 32-bit optional header.
    Pe32,
    UnknownOptionalHeader {
        magic: u16,
    },
}

impl PeClass {
    pub(crate) fn is_target(self) -> bool {
        matches!(self, Self::Pe32PlusX64)
    }
}

fn read_u16(image: &[u8], offset: usize) -> Option<u16> {
    let bytes = image.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(image: &[u8], offset: usize) -> Option<u32> {
    let bytes = image.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// True when the image starts with the DOS `MZ` marker (cheap pre-filter
/// used before committing to a full parse).
pub(crate) fn looks_like_pe(image: &[u8]) -> bool {
    image.starts_with(&DOS_MAGIC)
}

/// Parse the DOS header, PE signature, COFF header, optional-header fields,
/// and (up to `MAX_PARSED_SECTIONS`) the section table.
pub(crate) fn parse(image: &[u8]) -> Result<PeImage, PeError> {
    if !looks_like_pe(image) || image.len() < E_LFANEW_OFFSET + 4 {
        return Err(PeError::Truncated);
    }
    let e_lfanew = read_u32(image, E_LFANEW_OFFSET).ok_or(PeError::Truncated)? as usize;
    let coff_offset = e_lfanew
        .checked_add(PE_SIGNATURE.len())
        .ok_or(PeError::BadSignature)?;
    if image.get(e_lfanew..coff_offset) != Some(&PE_SIGNATURE[..]) {
        return Err(PeError::BadSignature);
    }
    if image.len() < coff_offset + COFF_HEADER_LEN {
        return Err(PeError::Truncated);
    }
    let coff = CoffHeader {
        machine: read_u16(image, coff_offset).ok_or(PeError::Truncated)?,
        section_count: read_u16(image, coff_offset + 2).ok_or(PeError::Truncated)?,
        timestamp: read_u32(image, coff_offset + 4).ok_or(PeError::Truncated)?,
        symbol_table_ptr: read_u32(image, coff_offset + 8).ok_or(PeError::Truncated)?,
        symbol_count: read_u32(image, coff_offset + 12).ok_or(PeError::Truncated)?,
        optional_header_size: read_u16(image, coff_offset + 16).ok_or(PeError::Truncated)?,
        characteristics: read_u16(image, coff_offset + 18).ok_or(PeError::Truncated)?,
    };
    // Optional header: magic @0, entry RVA @16, image base @24 (PE32+) or
    // @28 (u32, PE32). Only the shared prefix is validated here.
    if coff.optional_header_size < 2 {
        return Err(PeError::MissingOptionalHeader);
    }
    let opt_offset = coff_offset + COFF_HEADER_LEN;
    let optional_magic = read_u16(image, opt_offset).ok_or(PeError::Truncated)?;
    let entry_rva = read_u32(image, opt_offset + 16).ok_or(PeError::Truncated)?;
    let image_base = match optional_magic {
        OPT_MAGIC_PE32PLUS => {
            u64::from(read_u32(image, opt_offset + 24).ok_or(PeError::Truncated)?)
                | (u64::from(read_u32(image, opt_offset + 28).ok_or(PeError::Truncated)?) << 32)
        }
        _ => u64::from(read_u32(image, opt_offset + 28).ok_or(PeError::Truncated)?),
    };
    let subsystem = read_u16(image, opt_offset + 68).ok_or(PeError::Truncated)?;

    // Section table follows the optional header.
    let table_offset = opt_offset + coff.optional_header_size as usize;
    let wanted = coff.section_count as usize;
    let parsed = wanted.min(MAX_PARSED_SECTIONS);
    let end = table_offset
        .checked_add(
            parsed
                .checked_mul(SECTION_HEADER_LEN)
                .ok_or(PeError::Truncated)?,
        )
        .ok_or(PeError::Truncated)?;
    if image.len() < end {
        return Err(PeError::Truncated);
    }
    let mut sections = [PeSectionEntry {
        name: [0; 8],
        virtual_size: 0,
        virtual_address: 0,
        raw_size: 0,
        raw_ptr: 0,
        characteristics: 0,
    }; MAX_PARSED_SECTIONS];
    for index in 0..parsed {
        let base = table_offset + index * SECTION_HEADER_LEN;
        let mut name = [0u8; 8];
        name.copy_from_slice(&image[base..base + 8]);
        sections[index] = PeSectionEntry {
            name,
            virtual_size: read_u32(image, base + 8).unwrap_or(0),
            virtual_address: read_u32(image, base + 12).unwrap_or(0),
            raw_size: read_u32(image, base + 16).unwrap_or(0),
            raw_ptr: read_u32(image, base + 20).unwrap_or(0),
            characteristics: read_u32(image, base + 36).unwrap_or(0),
        };
    }
    Ok(PeImage {
        coff,
        optional_magic,
        entry_rva,
        image_base,
        subsystem,
        sections,
        sections_len: parsed,
    })
}

/// Human-readable subsystem tag for evidence lines and tests.
pub(crate) fn subsystem_name(subsystem: u16) -> &'static str {
    match subsystem {
        2 => "windows-gui",
        3 => "console",
        _ => "other",
    }
}

/// Architecture tag mirroring the COFF machine word.
pub(crate) fn machine_name(machine: u16) -> &'static str {
    match machine {
        MACHINE_I386 => "i386",
        MACHINE_X86_64 => "x86_64",
        MACHINE_AARCH64 => "aarch64",
        _ => "unknown",
    }
}

/// Hand-built golden minimal x86_64 PE32+ image: DOS header with
/// `e_lfanew = 0x80`, PE signature, COFF (2 sections), a standard
/// 240-byte PE32+ optional header (console subsystem, base
/// 0x140000000, entry 0x1000), and `.text` + `.data` section headers.
/// Shared with sibling modules' integration tests.
#[cfg(test)]
pub(crate) fn golden_pe32plus_fixture() -> Vec<u8> {
    golden_pe32plus_with_sections(2)
}

/// Variant with `count` real section headers (exercises extraction caps).
#[cfg(test)]
fn golden_pe32plus_with_sections(count: usize) -> Vec<u8> {
    const OPT_HEADER_SIZE: u16 = 240;

    fn push_section(
        image: &mut Vec<u8>,
        name: &[u8; 8],
        virtual_size: u32,
        virtual_address: u32,
        raw_size: u32,
        raw_ptr: u32,
        characteristics: u32,
    ) {
        image.extend_from_slice(name);
        // virtual_size, virtual_address, raw_size, raw_ptr,
        // reloc-pointer, line-number pointer.
        for value in [virtual_size, virtual_address, raw_size, raw_ptr, 0, 0] {
            image.extend_from_slice(&value.to_le_bytes());
        }
        // NumberOfRelocations, NumberOfLinenumbers, then characteristics
        // at +36 — full 40-byte section header.
        image.extend_from_slice(&0u16.to_le_bytes());
        image.extend_from_slice(&0u16.to_le_bytes());
        image.extend_from_slice(&characteristics.to_le_bytes());
    }

    let mut image = Vec::new();
    let mut dos = [0u8; 64];
    dos[0] = b'M';
    dos[1] = b'Z';
    dos[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&0x80u32.to_le_bytes());
    image.extend_from_slice(&dos);
    image.resize(0x80, 0);
    image.extend_from_slice(&PE_SIGNATURE);
    let mut coff = [0u8; COFF_HEADER_LEN];
    coff[0..2].copy_from_slice(&MACHINE_X86_64.to_le_bytes());
    coff[2..4].copy_from_slice(&(count as u16).to_le_bytes());
    coff[4..8].copy_from_slice(&0x12345678u32.to_le_bytes());
    coff[16..18].copy_from_slice(&OPT_HEADER_SIZE.to_le_bytes());
    coff[18..20].copy_from_slice(&0x22u16.to_le_bytes());
    image.extend_from_slice(&coff);
    let mut opt = [0u8; OPT_HEADER_SIZE as usize];
    opt[0..2].copy_from_slice(&OPT_MAGIC_PE32PLUS.to_le_bytes());
    opt[16..20].copy_from_slice(&0x1000u32.to_le_bytes());
    opt[24..28].copy_from_slice(&0x40000000u32.to_le_bytes());
    opt[28..32].copy_from_slice(&1u32.to_le_bytes());
    opt[68..70].copy_from_slice(&3u16.to_le_bytes());
    image.extend_from_slice(&opt);
    for index in 0..count {
        if index == 0 {
            push_section(
                &mut image,
                b".text\0\0\0",
                0x200,
                0x1000,
                0x200,
                0x400,
                0x6000_0020,
            );
        } else if index == 1 {
            push_section(
                &mut image,
                b".data\0\0\0",
                0x100,
                0x2000,
                0x200,
                0x600,
                0xC000_0040,
            );
        } else {
            let mut name = *b".s N\0\0\0\0";
            name[3] = b'0' + (index as u8 % 10);
            push_section(&mut image, &name, 0x10, 0x3000, 0x10, 0x800, 0x4200_0040);
        }
    }
    image
}

mod tests {
    use super::*;

    #[test]
    fn golden_pe32plus_classifies_and_extracts_sections() {
        let fixture = golden_pe32plus_fixture();
        let parsed = parse(&fixture).expect("golden fixture parses");
        assert!(parsed.classify().is_target());
        assert_eq!(
            parsed.classify(),
            PeClass::Pe32PlusX64,
            "x86_64 PE32+ is the roadmap target shape"
        );
        assert_eq!(parsed.coff.machine, MACHINE_X86_64);
        assert_eq!(parsed.coff.section_count, 2);
        assert_eq!(parsed.coff.timestamp, 0x12345678);
        assert_eq!(parsed.coff.optional_header_size, 240);
        assert_eq!(parsed.optional_magic, OPT_MAGIC_PE32PLUS);
        assert_eq!(parsed.entry_rva, 0x1000);
        assert_eq!(parsed.image_base, 0x1_4000_0000);
        assert_eq!(parsed.subsystem, 3);
        let sections = parsed.section_slice();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name_str(), Some(".text"));
        assert_eq!(sections[0].virtual_address, 0x1000);
        assert_eq!(sections[0].raw_ptr, 0x400);
        assert_eq!(sections[1].name_str(), Some(".data"));
        assert_eq!(sections[1].virtual_size, 0x100);
        assert_eq!(sections[1].characteristics, 0xC000_0040);
        assert_eq!(subsystem_name(3), "console");
        assert_eq!(machine_name(MACHINE_X86_64), "x86_64");
    }

    #[test]
    fn rejects_truncated_and_bad_signatures() {
        assert_eq!(parse(&[]), Err(PeError::Truncated));
        assert_eq!(parse(b"M"), Err(PeError::Truncated));
        // e_lfanew pointing past the buffer.
        let mut short = golden_pe32plus_fixture();
        short.truncate(0x84);
        assert_eq!(parse(&short), Err(PeError::Truncated));
        // e_lfanew inside the buffer but signature missing.
        let mut corrupt = golden_pe32plus_fixture();
        let at = 0x80;
        corrupt[at..at + 4].copy_from_slice(&[0x50, 0x45, 0x01, 0x00]);
        assert_eq!(parse(&corrupt), Err(PeError::BadSignature));
        // e_lfanew beyond the end.
        let mut far = [0u8; 64];
        far[..2].copy_from_slice(&DOS_MAGIC);
        far[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&far), Err(PeError::BadSignature));
    }

    #[test]
    fn rejects_wrong_machines() {
        let mut i386 = golden_pe32plus_fixture();
        i386[0x84..0x86].copy_from_slice(&0x14cu16.to_le_bytes());
        let parsed_i386 = parse(&i386).expect("i386 header still parses");
        assert_eq!(
            parsed_i386.classify(),
            PeClass::UnsupportedMachine { machine: 0x14c }
        );
        assert!(!parsed_i386.classify().is_target());

        let mut arm64 = golden_pe32plus_fixture();
        arm64[0x84..0x86].copy_from_slice(&0xaa64u16.to_le_bytes());
        assert_eq!(
            parse(&arm64).expect("arm64").classify(),
            PeClass::UnsupportedMachine {
                machine: MACHINE_AARCH64
            }
        );
    }

    #[test]
    fn rejects_pe32_optional_header_on_x86_64() {
        let mut pe32 = golden_pe32plus_fixture();
        // Optional header starts at 0x80(sig) + 4 + 20(COFF) = 0x98.
        pe32[0x98..0x9a].copy_from_slice(&OPT_MAGIC_PE32.to_le_bytes());
        let parsed = parse(&pe32).expect("PE32 header still parses");
        assert_eq!(parsed.classify(), PeClass::Pe32);
        assert!(!parsed.classify().is_target());
    }

    #[test]
    fn section_table_extraction_is_capped() {
        // 20 real section headers; extraction caps at MAX_PARSED_SECTIONS.
        let fixture = golden_pe32plus_with_sections(20);
        let parsed = parse(&fixture).expect("capped table parses");
        assert_eq!(parsed.coff.section_count, 20);
        assert_eq!(parsed.sections_len, MAX_PARSED_SECTIONS.min(20));
        assert_eq!(parsed.section_slice()[0].name_str(), Some(".text"));
        // A declared count larger than the actual table is corrupt.
        let mut overclaim = golden_pe32plus_fixture();
        overclaim[0x86..0x88].copy_from_slice(&20u16.to_le_bytes());
        assert_eq!(parse(&overclaim), Err(PeError::Truncated));
    }

    #[test]
    fn non_ascii_section_names_are_honest_none() {
        let mut fixture = golden_pe32plus_fixture();
        // Section table begins after the 240-byte optional header: 0x188.
        fixture[0x188..0x190].copy_from_slice(&[0xff; 8]);
        let parsed = parse(&fixture).expect("parses");
        assert_eq!(parsed.section_slice()[0].name_str(), None);
    }
}
