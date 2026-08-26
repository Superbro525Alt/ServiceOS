//! Guest executable image format classification for the runtime spawn path.
//!
//! Mirrors the kernel loader's fallback order (`flat` v2 → `flat` v1 → raw
//! ELF64) so the runtime-service can pre-flight guest images before handing
//! them to the manager-mediated `LaunchImage` path. This is a pure,
//! host-testable gate: the kernel remains the authority for segment tables,
//! user-window containment, and W^X policy at map time.

/// Runtime-service-local workload marker for guest-image execution. Lives
/// outside the shared-ABI `RuntimeWorkloadKind` enum until that contract
/// grows an `Exec` variant; unknown values coerce to `Inspect` on senders.
pub(crate) const EXEC_GUEST_WORKLOAD: u32 = 5;

const FLAT_IMAGE_MAGIC: [u8; 8] = *b"SOSUIMG\0";
const FLAT_HEADER_LEN_V1: u32 = 72;
const FLAT_HEADER_LEN_V2: u32 = 280;
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LSB: u8 = 1;
const ELF_VERSION_CURRENT: u8 = 1;
const ELF_TYPE_EXEC: u16 = 2;
const ELF_PROGRAM_HEADER_LEN: usize = 56;
const MAX_ELF_PROGRAM_HEADERS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageFormat {
    /// v2 flat image with extension fields (dependencies, segment table).
    FlatV2,
    /// Legacy v1 flat image.
    FlatV1,
    /// Static ELF64 executable loaded from its PT_LOAD segments.
    RawElf64,
}

impl ImageFormat {
    #[cfg(test)]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::FlatV2 => "flat-v2",
            Self::FlatV1 => "flat-v1",
            Self::RawElf64 => "elf64",
        }
    }

    /// Stable numeric discriminator for audit log payloads.
    pub(crate) fn marker(self) -> u32 {
        match self {
            Self::FlatV2 => 1,
            Self::FlatV1 => 2,
            Self::RawElf64 => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageParseError {
    Truncated,
    UnknownFormat,
    UnsupportedVariant,
    /// DOS/PE image detected and classified by the Windows-runtime
    /// groundwork (`crate::pe`) — refused as unsupported because no WinAPI
    /// ABI layer exists yet.
    WindowsPe,
}

/// Classify a guest image exactly as the kernel loader would.
pub(crate) fn classify_image(image: &[u8]) -> Result<ImageFormat, ImageParseError> {
    if starts_with_flat_magic(image) {
        return classify_flat_image(image);
    }
    if crate::pe::looks_like_pe(image) {
        // Detection only: any parseable PE is honestly refused; a corrupt
        // MZ blob stays UnknownFormat.
        return match crate::pe::parse(image) {
            Ok(_) => Err(ImageParseError::WindowsPe),
            Err(_) => Err(ImageParseError::UnknownFormat),
        };
    }
    if starts_with(&image, &ELF_MAGIC) {
        return classify_elf64_image(image);
    }
    Err(ImageParseError::UnknownFormat)
}

fn starts_with(image: &[u8], prefix: &[u8]) -> bool {
    image.len() >= prefix.len() && &image[..prefix.len()] == prefix
}

fn starts_with_flat_magic(image: &[u8]) -> bool {
    starts_with(image, &FLAT_IMAGE_MAGIC)
}

fn classify_flat_image(image: &[u8]) -> Result<ImageFormat, ImageParseError> {
    // Flat layout: magic[0..8], then version u32 @8 and header_len u32 @12.
    let header_len = read_u32_le(image, 12).ok_or(ImageParseError::Truncated)?;
    let format = match header_len {
        FLAT_HEADER_LEN_V2 => ImageFormat::FlatV2,
        FLAT_HEADER_LEN_V1 => ImageFormat::FlatV1,
        _ => return Err(ImageParseError::UnsupportedVariant),
    };
    let abi_version = read_u32_le(image, 8).ok_or(ImageParseError::Truncated)?;
    if abi_version != 1 {
        return Err(ImageParseError::UnsupportedVariant);
    }
    Ok(format)
}

fn classify_elf64_image(image: &[u8]) -> Result<ImageFormat, ImageParseError> {
    if image.len() < 64 {
        return Err(ImageParseError::Truncated);
    }
    if image[4] != ELF_CLASS_64 || image[5] != ELF_DATA_LSB || image[6] != ELF_VERSION_CURRENT {
        return Err(ImageParseError::UnsupportedVariant);
    }
    let elf_type = read_u16_le(image, 16).ok_or(ImageParseError::Truncated)?;
    // Static executables only, mirroring the kernel loader's ET_EXEC gate:
    // dynamic/PIE images need a relocation story that does not exist yet.
    if elf_type != ELF_TYPE_EXEC {
        return Err(ImageParseError::UnsupportedVariant);
    }
    let phoff = u64_le(image, 32).ok_or(ImageParseError::Truncated)? as usize;
    let phentsize = read_u16_le(image, 54).ok_or(ImageParseError::Truncated)? as usize;
    let phnum = read_u16_le(image, 56).ok_or(ImageParseError::Truncated)? as usize;
    if phentsize != ELF_PROGRAM_HEADER_LEN || phnum == 0 || phnum > MAX_ELF_PROGRAM_HEADERS {
        return Err(ImageParseError::UnsupportedVariant);
    }
    let mut load_segments = 0usize;
    for index in 0..phnum {
        let base = phoff
            .checked_add(index * phentsize)
            .ok_or(ImageParseError::Truncated)?;
        let header = image
            .get(base..base + phentsize)
            .ok_or(ImageParseError::Truncated)?;
        let segment_type = read_u32_le(header, 0).ok_or(ImageParseError::Truncated)?;
        if segment_type == 1 {
            load_segments += 1;
        }
    }
    if load_segments == 0 {
        return Err(ImageParseError::UnsupportedVariant);
    }
    Ok(ImageFormat::RawElf64)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ElfLoadSegment {
    pub(crate) writable: bool,
    pub(crate) executable: bool,
}

/// Test-side summary of the PT_LOAD permission split of a static ELF64
/// image, mirroring what the kernel will enforce per page at map time
/// (W^X flags straight from the program headers).
#[cfg(test)]
pub(crate) fn elf_load_segment_summary(
    image: &[u8],
) -> Result<(usize, [ElfLoadSegment; 8]), ImageParseError> {
    if classify_elf64_image(image).is_err() {
        return Err(classify_elf64_image(image).unwrap_err());
    }
    let phoff = u64_le(image, 32).ok_or(ImageParseError::Truncated)? as usize;
    let phnum = read_u16_le(image, 56).ok_or(ImageParseError::Truncated)? as usize;
    let mut segments = [ElfLoadSegment {
        writable: false,
        executable: false,
    }; 8];
    let mut count = 0usize;
    for index in 0..phnum {
        let base = phoff
            .checked_add(index * ELF_PROGRAM_HEADER_LEN)
            .ok_or(ImageParseError::Truncated)?;
        let header = image
            .get(base..base + ELF_PROGRAM_HEADER_LEN)
            .ok_or(ImageParseError::Truncated)?;
        if read_u32_le(header, 0).ok_or(ImageParseError::Truncated)? != 1 {
            continue;
        }
        let flags = read_u32_le(header, 4).ok_or(ImageParseError::Truncated)?;
        if count < segments.len() {
            segments[count] = ElfLoadSegment {
                writable: flags & 2 != 0,
                executable: flags & 1 != 0,
            };
        }
        count += 1;
    }
    Ok((count, segments))
}

fn read_u32_le(image: &[u8], offset: usize) -> Option<u32> {
    let bytes = image.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u16_le(image: &[u8], offset: usize) -> Option<u16> {
    let bytes = image.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn u64_le(image: &[u8], offset: usize) -> Option<u64> {
    let bytes = image.get(offset..offset + 8)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serviceos_userspace_runtime as rt;

    fn flat_header(header_len: u32) -> Vec<u8> {
        let mut image = vec![0u8; header_len as usize + 16];
        image[..8].copy_from_slice(&FLAT_IMAGE_MAGIC);
        image[8..12].copy_from_slice(&1u32.to_le_bytes());
        image[12..16].copy_from_slice(&header_len.to_le_bytes());
        image
    }

    #[test]
    fn classifies_flat_v2_and_v1_by_header_length() {
        assert_eq!(classify_image(&flat_header(280)), Ok(ImageFormat::FlatV2));
        assert_eq!(classify_image(&flat_header(72)), Ok(ImageFormat::FlatV1));
    }

    #[test]
    fn rejects_flat_images_with_unknown_abi_version_or_header_length() {
        let mut image = flat_header(280);
        image[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            classify_image(&image),
            Err(ImageParseError::UnsupportedVariant)
        );
        assert_eq!(
            classify_image(&flat_header(96)),
            Err(ImageParseError::UnsupportedVariant)
        );
    }

    /// Golden minimal ELF64 header modeled on the kernel loader's own test
    /// fixtures: ET_EXEC x86_64 LSB with one R-X PT_LOAD and one RW PT_LOAD.
    #[test]
    fn classifies_static_elf64_with_load_segments() {
        let image = golden_static_elf64();
        assert_eq!(classify_image(&image), Ok(ImageFormat::RawElf64));
        let (count, segments) = elf_load_segment_summary(&image).expect("summary");
        assert_eq!(count, 2);
        assert!(segments[0].executable && !segments[0].writable);
        assert!(segments[1].writable && !segments[1].executable);
    }

    #[test]
    fn rejects_truncated_unknown_and_non_exec_elf64() {
        assert_eq!(classify_image(&[]), Err(ImageParseError::UnknownFormat));
        assert_eq!(classify_image(&ELF_MAGIC), Err(ImageParseError::Truncated));

        let mut shared_object = golden_static_elf64();
        shared_object[16..18].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            classify_image(&shared_object),
            Err(ImageParseError::UnsupportedVariant),
            "ET_DYN (dynamic/PIE) stays unsupported until relocation support lands"
        );

        let mut corrupt = golden_static_elf64();
        corrupt[56..58].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            classify_image(&corrupt),
            Err(ImageParseError::UnsupportedVariant)
        );
    }

    #[test]
    fn exec_workload_marker_does_not_collide_with_shared_abi_kinds() {
        let known = [
            rt::RuntimeWorkloadKind::Inspect as u32,
            rt::RuntimeWorkloadKind::Env as u32,
            rt::RuntimeWorkloadKind::Mounts as u32,
            rt::RuntimeWorkloadKind::Cat as u32,
        ];
        assert!(!known.contains(&EXEC_GUEST_WORKLOAD));
    }

    #[test]
    fn classify_detects_and_refuses_windows_pe() {
        let fixture = crate::pe::golden_pe32plus_fixture();
        assert_eq!(classify_image(&fixture), Err(ImageParseError::WindowsPe));

        // A corrupt MZ blob is not claimed as a PE.
        let mut corrupt = fixture.clone();
        corrupt[0x80..0x84].copy_from_slice(&[0u8; 4]);
        assert_eq!(
            classify_image(&corrupt),
            Err(ImageParseError::UnknownFormat)
        );
    }

    fn golden_static_elf64() -> Vec<u8> {
        let phnum: u16 = 3;
        let phoff: u64 = 64;
        let mut image = vec![0u8; 64 + phnum as usize * ELF_PROGRAM_HEADER_LEN];
        image[..4].copy_from_slice(&ELF_MAGIC);
        image[4] = ELF_CLASS_64;
        image[5] = ELF_DATA_LSB;
        image[6] = ELF_VERSION_CURRENT;
        image[16..18].copy_from_slice(&ELF_TYPE_EXEC.to_le_bytes());
        image[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        image[32..40].copy_from_slice(&phoff.to_le_bytes());
        image[54..56].copy_from_slice(&(ELF_PROGRAM_HEADER_LEN as u16).to_le_bytes());
        image[56..58].copy_from_slice(&phnum.to_le_bytes());

        let mut program = |index: usize, p_type: u32, flags: u32| {
            let base = 64 + index * ELF_PROGRAM_HEADER_LEN;
            image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
            image[base + 4..base + 8].copy_from_slice(&flags.to_le_bytes());
        };
        program(0, 1, 5); // PT_LOAD r-x
        program(1, 1, 6); // PT_LOAD rw-
        program(2, 0x6474_e551, 6); // PT_GNU_STACK rw- (NX stack)
        image
    }
}
