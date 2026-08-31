mod loader;
mod runtime;
mod spawn;
mod types;

pub use loader::{ElfMachine, load_flat_image, load_image, parse_flat_image};
pub use runtime::{
    UserRuntime, arch_hooks, current_task_syscall_abi, image_resolver, initialize_runtime,
    loaded_image_for, record_loaded_image, register_arch_hooks, register_image_resolver, runtime,
};
pub use spawn::{
    current_task, mark_current_thread_exited, mark_current_thread_faulted, spawn_builtin_task,
    spawn_image_bytes, spawn_image_bytes_with_abi,
};
pub use types::{
    AddressSpacePreparationError, FlatDependencyRecord, FlatImageHeader, FlatSegmentRecord,
    KERNEL_ABI_VERSION, LoadError, LoadedLibraryRecord, LoadedUserImage, MAX_FLAT_DEPENDENCIES,
    MAX_FLAT_SEGMENTS, PreparedUserAddressSpace, SpawnError, SpawnedUserTask, TaskExitStatus,
    UserArchHooks, UserThreadLaunch, flat_image_policy,
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

    #[test]
    fn parse_flat_image_accepts_cross_builder_native_artifact_shape() {
        const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
        const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;
        let message = b"hello from builder";
        let code_len = 31usize + message.len();
        let file_size = code_len as u64;

        let image = build_image(
            1,
            IMAGE_BASE,
            0,
            file_size,
            file_size,
            file_size,
            file_size,
            USER_STACK_TOP,
            &alloc::vec![0u8; code_len],
        );

        let header = parse_flat_image(&image).expect("builder-style image should parse");
        assert_eq!(header.entry_offset, 0);
        assert_eq!(header.file_size, code_len);
        assert_eq!(header.image_base, VirtualAddress::new(IMAGE_BASE));
        assert_eq!(header.user_stack_top, VirtualAddress::new(USER_STACK_TOP));
    }
}
