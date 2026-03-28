mod loader;
mod runtime;
mod spawn;
mod types;

pub use loader::{load_flat_image, parse_flat_image};
pub use runtime::{
    UserRuntime, arch_hooks, initialize_runtime, register_arch_hooks, register_image_resolver,
    runtime,
};
pub use spawn::{
    current_task, mark_current_thread_exited, mark_current_thread_faulted, spawn_builtin_task,
    spawn_image_bytes,
};
pub use types::{
    AddressSpacePreparationError, FlatImageHeader, LoadError, LoadedUserImage,
    PreparedUserAddressSpace, SpawnError, SpawnedUserTask, TaskExitStatus, UserArchHooks,
    UserThreadLaunch,
};

#[cfg(test)]
mod tests {
    use super::types::{FLAT_IMAGE_HEADER_LEN, flat_image_magic};
    use super::*;
    use crate::memory::VirtualAddress;
    use alloc::vec::Vec;

    fn build_image(
        abi_version: u32,
        image_base: u64,
        entry_offset: u64,
        file_size: u64,
        executable_limit: u64,
        writable_offset: u64,
        memory_size: u64,
        user_stack_top: u64,
        code_bytes: &[u8],
    ) -> Vec<u8> {
        let mut image = Vec::new();
        image.extend_from_slice(&flat_image_magic());
        image.extend_from_slice(&abi_version.to_le_bytes());
        image.extend_from_slice(&(FLAT_IMAGE_HEADER_LEN as u32).to_le_bytes());
        image.extend_from_slice(&image_base.to_le_bytes());
        image.extend_from_slice(&entry_offset.to_le_bytes());
        image.extend_from_slice(&file_size.to_le_bytes());
        image.extend_from_slice(&executable_limit.to_le_bytes());
        image.extend_from_slice(&writable_offset.to_le_bytes());
        image.extend_from_slice(&memory_size.to_le_bytes());
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
            4,
            4,
            4,
            0x7fff_ffff_f000,
            &[1, 2, 3, 4],
        );

        let header = parse_flat_image(&image).expect("header should parse");
        assert_eq!(header.abi_version, 1);
        assert_eq!(header.image_base, VirtualAddress::new(0x4000_0000_0000));
        assert_eq!(header.entry_offset, 0x20);
        assert_eq!(header.file_size, 4);
        assert_eq!(header.executable_limit, 4);
        assert_eq!(header.writable_offset, 4);
        assert_eq!(header.memory_size, 4);
        assert_eq!(header.user_stack_top, VirtualAddress::new(0x7fff_ffff_f000));
    }

    #[test]
    fn parse_flat_image_rejects_misaligned_addresses() {
        let image = build_image(
            1,
            0x4000_0000_0001,
            0,
            4,
            4,
            4,
            4,
            0x7fff_ffff_f000,
            &[1, 2, 3, 4],
        );
        assert_eq!(parse_flat_image(&image), Err(LoadError::AddressAlignment));

        let image = build_image(
            1,
            0x4000_0000_0000,
            0,
            4,
            4,
            4,
            4,
            0x7fff_ffff_f001,
            &[1, 2, 3, 4],
        );
        assert_eq!(parse_flat_image(&image), Err(LoadError::AddressAlignment));
    }

    #[test]
    fn parse_flat_image_rejects_wrong_abi_and_truncation() {
        let unsupported = build_image(
            2,
            0x4000_0000_0000,
            0,
            4,
            4,
            4,
            4,
            0x7fff_ffff_f000,
            &[1, 2, 3, 4],
        );
        assert_eq!(
            parse_flat_image(&unsupported),
            Err(LoadError::UnsupportedAbi)
        );

        let truncated = build_image(
            1,
            0x4000_0000_0000,
            0,
            8,
            8,
            8,
            8,
            0x7fff_ffff_f000,
            &[1, 2, 3, 4],
        );
        assert_eq!(parse_flat_image(&truncated), Err(LoadError::Truncated));
    }
}
