#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::global_asm;

    use spin::Mutex;

    use crate::mmu::{OwnedPageTable, current_page_table_root, load_page_table_root};
    use serviceos_kernel_core::{
        memory::{
            self, Frame, MappingError, MappingFlags, PageMapper, PageSize, PhysicalAddress,
            VirtualAddress,
        },
        task::{AddressSpaceId, ThreadId},
        user::{
            self, AddressSpacePreparationError, ElfMachine, LoadError, PreparedUserAddressSpace,
            UserArchHooks, UserThreadLaunch,
        },
    };

    const MAX_USER_THREADS: usize = 32;
    const MAX_USER_ADDRESS_SPACES: usize = 32;

    global_asm!(
        r#"
.global serviceos_aarch64_resume_user
serviceos_aarch64_resume_user:
    stp x19, x20, [sp, #-16]!
    stp x21, x22, [sp, #-16]!
    stp x23, x24, [sp, #-16]!
    stp x25, x26, [sp, #-16]!
    stp x27, x28, [sp, #-16]!
    stp x29, x30, [sp, #-16]!
    mov x9, sp
    adrp x10, serviceos_aarch64_kernel_return_sp
    add x10, x10, :lo12:serviceos_aarch64_kernel_return_sp
    str x9, [x10]
    adrp x10, serviceos_aarch64_current_context
    add x10, x10, :lo12:serviceos_aarch64_current_context
    str x0, [x10]
    // Re-publish (x0, x1) into the raw_syscall result slot at [sp-16, sp-8]
    // only when the thread was suspended by the scheduler (the sync stub
    // arms resume_publish on its kernel-continuation exit). An IRQ-preempted
    // thread resumes with resume_publish clear so its pending result slot —
    // possibly not yet consumed by the svc caller — is left untouched.
    ldr x12, [x0, #0x120]
    cbz x12, 2f
    ldr x9, [x0, #0xF8]
    sub x9, x9, #16
    ldr x10, [x0]
    ldr x11, [x0, #8]
    str x10, [x9]
    str x11, [x9, #8]
    str xzr, [x0, #0x120]
2:

    mov x30, x0
    ldr x12, [x30, #0xF8]
    msr sp_el0, x12
    ldr x12, [x30, #0x100]
    msr elr_el1, x12
    ldr x12, [x30, #0x108]
    msr spsr_el1, x12
    ldp x0, x1, [x30, #0x00]
    ldp x2, x3, [x30, #0x10]
    ldp x4, x5, [x30, #0x20]
    ldp x6, x7, [x30, #0x30]
    ldp x8, x9, [x30, #0x40]
    ldp x10, x11, [x30, #0x50]
    ldp x12, x13, [x30, #0x60]
    ldp x14, x15, [x30, #0x70]
    ldp x16, x17, [x30, #0x80]
    ldp x18, x19, [x30, #0x90]
    ldp x20, x21, [x30, #0xA0]
    ldp x22, x23, [x30, #0xB0]
    ldp x24, x25, [x30, #0xC0]
    ldp x26, x27, [x30, #0xD0]
    ldp x28, x29, [x30, #0xE0]
    ldr x30, [x30, #0xF0]
    eret

.global serviceos_aarch64_lower_el_sync
serviceos_aarch64_lower_el_sync:
    stp x10, x11, [sp, #-16]!
    adrp x10, serviceos_aarch64_current_context
    add x10, x10, :lo12:serviceos_aarch64_current_context
    ldr x11, [x10]
    stp x0, x1, [x11, #0x00]
    stp x2, x3, [x11, #0x10]
    stp x4, x5, [x11, #0x20]
    stp x6, x7, [x11, #0x30]
    stp x8, x9, [x11, #0x40]
    stp x12, x13, [x11, #0x60]
    ldr x12, [sp, #0]
    ldr x13, [sp, #8]
    stp x12, x13, [x11, #0x50]
    stp x14, x15, [x11, #0x70]
    stp x16, x17, [x11, #0x80]
    stp x18, x19, [x11, #0x90]
    stp x20, x21, [x11, #0xA0]
    stp x22, x23, [x11, #0xB0]
    stp x24, x25, [x11, #0xC0]
    stp x26, x27, [x11, #0xD0]
    stp x28, x29, [x11, #0xE0]
    str x30, [x11, #0xF0]
    mrs x12, sp_el0
    str x12, [x11, #0xF8]
    mrs x12, elr_el1
    str x12, [x11, #0x100]
    mrs x12, spsr_el1
    str x12, [x11, #0x108]
    mrs x12, esr_el1
    str x12, [x11, #0x110]
    mrs x12, far_el1
    str x12, [x11, #0x118]
    add sp, sp, #16

    mov x0, x11
    bl serviceos_aarch64_handle_user_sync
    cbz x0, 1f

    // Scheduler suspension (block/yield/exit): arm resume_publish so the
    // next resume_user re-delivers the saved (x0, x1) pair through the
    // raw_syscall result slot, matching the direct-return memory channel.
    adrp x10, serviceos_aarch64_current_context
    add x10, x10, :lo12:serviceos_aarch64_current_context
    ldr x10, [x10]
    mov x11, #1
    str x11, [x10, #0x120]

    adrp x10, serviceos_aarch64_kernel_return_sp
    add x10, x10, :lo12:serviceos_aarch64_kernel_return_sp
    ldr x9, [x10]
    mov sp, x9
    ldp x29, x30, [sp], #16
    ldp x27, x28, [sp], #16
    ldp x25, x26, [sp], #16
    ldp x23, x24, [sp], #16
    ldp x21, x22, [sp], #16
    ldp x19, x20, [sp], #16
    ret

1:
    // x0-x17 were clobbered by serviceos_aarch64_handle_user_sync (AAPCS
    // caller-saved), so x11 no longer holds the context pointer. Reload it
    // from the current_context global saved by serviceos_aarch64_resume_user.
    adrp x30, serviceos_aarch64_current_context
    add x30, x30, :lo12:serviceos_aarch64_current_context
    ldr x30, [x30]
    ldr x12, [x30, #0xF8]
    msr sp_el0, x12
    ldr x12, [x30, #0x100]
    msr elr_el1, x12
    ldr x12, [x30, #0x108]
    msr spsr_el1, x12
    ldp x0, x1, [x30, #0x00]
    ldp x2, x3, [x30, #0x10]
    ldp x4, x5, [x30, #0x20]
    ldp x6, x7, [x30, #0x30]
    ldp x8, x9, [x30, #0x40]
    ldp x10, x11, [x30, #0x50]
    ldp x12, x13, [x30, #0x60]
    ldp x14, x15, [x30, #0x70]
    ldp x16, x17, [x30, #0x80]
    ldp x18, x19, [x30, #0x90]
    ldp x20, x21, [x30, #0xA0]
    ldp x22, x23, [x30, #0xB0]
    ldp x24, x25, [x30, #0xC0]
    ldp x26, x27, [x30, #0xD0]
    ldp x28, x29, [x30, #0xE0]
    ldr x30, [x30, #0xF0]
    eret
"#
    );

    unsafe extern "C" {
        fn serviceos_aarch64_resume_user(context: *const SavedUserContext);
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct SavedUserContext {
        pub x0: u64,
        pub x1: u64,
        pub x2: u64,
        pub x3: u64,
        pub x4: u64,
        pub x5: u64,
        pub x6: u64,
        pub x7: u64,
        pub x8: u64,
        pub x9: u64,
        pub x10: u64,
        pub x11: u64,
        pub x12: u64,
        pub x13: u64,
        pub x14: u64,
        pub x15: u64,
        pub x16: u64,
        pub x17: u64,
        pub x18: u64,
        pub x19: u64,
        pub x20: u64,
        pub x21: u64,
        pub x22: u64,
        pub x23: u64,
        pub x24: u64,
        pub x25: u64,
        pub x26: u64,
        pub x27: u64,
        pub x28: u64,
        pub x29: u64,
        pub x30: u64,
        pub sp_el0: u64,
        pub elr_el1: u64,
        pub spsr_el1: u64,
        pub esr_el1: u64,
        pub far_el1: u64,
        /// Set by the sync stub when a syscall suspends the thread through
        /// the scheduler (block/yield/exit); resume_user re-publishes the
        /// saved (x0, x1) result pair into the raw_syscall memory slot only
        /// when this is armed. An IRQ preemption save clears it so a
        /// preempted thread's pending result slot is left untouched.
        pub resume_publish: u64,
    }

    impl SavedUserContext {
        // The resume/trap asm addresses these fields by fixed byte offsets
        // (x30 at 0xF0, sp_el0 at 0xF8, elr_el1 at 0x100, spsr_el1 at 0x108,
        // esr_el1 at 0x110, far_el1 at 0x118, resume_publish at 0x120); keep
        // the repr(C) layout honest.
        const _LAYOUT_MATCHES_ASM: () = {
            assert!(core::mem::size_of::<Self>() == 37 * core::mem::size_of::<u64>());
            assert!(core::mem::offset_of!(Self, x30) == 0xF0);
            assert!(core::mem::offset_of!(Self, sp_el0) == 0xF8);
            assert!(core::mem::offset_of!(Self, elr_el1) == 0x100);
            assert!(core::mem::offset_of!(Self, spsr_el1) == 0x108);
            assert!(core::mem::offset_of!(Self, esr_el1) == 0x110);
            assert!(core::mem::offset_of!(Self, far_el1) == 0x118);
            assert!(core::mem::offset_of!(Self, resume_publish) == 0x120);
        };

        fn initial(entry_point: u64, user_stack_pointer: u64) -> Self {
            Self {
                x0: 0,
                x1: 0,
                x2: 0,
                x3: 0,
                x4: 0,
                x5: 0,
                x6: 0,
                x7: 0,
                x8: 0,
                x9: 0,
                x10: 0,
                x11: 0,
                x12: 0,
                x13: 0,
                x14: 0,
                x15: 0,
                x16: 0,
                x17: 0,
                x18: 0,
                x19: 0,
                x20: 0,
                x21: 0,
                x22: 0,
                x23: 0,
                x24: 0,
                x25: 0,
                x26: 0,
                x27: 0,
                x28: 0,
                x29: 0,
                x30: 0,
                sp_el0: user_stack_pointer,
                elr_el1: entry_point,
                spsr_el1: 0,
                esr_el1: 0,
                far_el1: 0,
                resume_publish: 0,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct SavedUserThread {
        thread_id: ThreadId,
        page_table_root: PhysicalAddress,
        context: SavedUserContext,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum UserLaunchError {
        UnknownThread,
        RuntimeCapacityExceeded,
        Mapping(MappingError),
        Load(LoadError),
        MemoryUnavailable,
    }

    impl From<MappingError> for UserLaunchError {
        fn from(error: MappingError) -> Self {
            Self::Mapping(error)
        }
    }

    impl From<LoadError> for UserLaunchError {
        fn from(error: LoadError) -> Self {
            Self::Load(error)
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct UserThreadRuntime {
        slots: [Option<SavedUserThread>; MAX_USER_THREADS],
    }

    impl UserThreadRuntime {
        const fn new() -> Self {
            Self {
                slots: [None; MAX_USER_THREADS],
            }
        }

        fn register_launch(&mut self, launch: UserThreadLaunch) -> Result<(), UserLaunchError> {
            for slot in &mut self.slots {
                if slot.is_none() {
                    *slot = Some(SavedUserThread {
                        thread_id: launch.thread_id,
                        page_table_root: launch.page_table_root,
                        context: SavedUserContext::initial(
                            launch.entry_point,
                            launch.user_stack_pointer,
                        ),
                    });
                    return Ok(());
                }
            }

            Err(UserLaunchError::RuntimeCapacityExceeded)
        }

        fn thread_mut(&mut self, thread_id: ThreadId) -> Option<&mut SavedUserThread> {
            self.slots
                .iter_mut()
                .flatten()
                .find(|thread| thread.thread_id == thread_id)
        }

        fn release_thread(&mut self, thread_id: ThreadId) {
            if let Some(slot) = self.slots.iter_mut().find(|slot| {
                slot.as_ref()
                    .is_some_and(|thread| thread.thread_id == thread_id)
            }) {
                *slot = None;
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct SavedAddressSpace {
        address_space_id: AddressSpaceId,
        page_table_root: PhysicalAddress,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct AddressSpaceRuntime {
        slots: [Option<SavedAddressSpace>; MAX_USER_ADDRESS_SPACES],
    }

    impl AddressSpaceRuntime {
        const fn new() -> Self {
            Self {
                slots: [None; MAX_USER_ADDRESS_SPACES],
            }
        }

        fn register(&mut self, address_space_id: AddressSpaceId, page_table_root: PhysicalAddress) {
            for slot in &mut self.slots {
                if slot.is_none() {
                    *slot = Some(SavedAddressSpace {
                        address_space_id,
                        page_table_root,
                    });
                    return;
                }
            }
            panic!("address space runtime capacity must cover bootstrap services");
        }

        fn root(&self, address_space_id: AddressSpaceId) -> Option<PhysicalAddress> {
            self.slots
                .iter()
                .flatten()
                .find(|entry| entry.address_space_id == address_space_id)
                .map(|entry| entry.page_table_root)
        }

        fn release(&mut self, address_space_id: AddressSpaceId) {
            if let Some(slot) = self.slots.iter_mut().find(|slot| {
                slot.as_ref()
                    .is_some_and(|entry| entry.address_space_id == address_space_id)
            }) {
                *slot = None;
            }
        }
    }

    #[unsafe(no_mangle)]
    static mut serviceos_aarch64_current_context: u64 = 0;
    #[unsafe(no_mangle)]
    static mut serviceos_aarch64_kernel_return_sp: u64 = 0;

    /// Exception stack storage; reserved for future EL1 exception-stack use.
    #[allow(dead_code)]
    #[repr(align(16))]
    struct ExceptionStack([u8; 16 * 1024]);

    #[allow(dead_code)]
    static EXCEPTION_STACK: ExceptionStack = ExceptionStack([0; 16 * 1024]);

    static USER_THREADS: Mutex<UserThreadRuntime> = Mutex::new(UserThreadRuntime::new());
    static ADDRESS_SPACES: Mutex<AddressSpaceRuntime> = Mutex::new(AddressSpaceRuntime::new());

    pub fn initialize() {
        // SAFETY: barrier-only inline assembly touches no memory or stack.
        unsafe {
            core::arch::asm!("isb", options(nomem, nostack));
        }
        user::register_arch_hooks(UserArchHooks {
            prepare_address_space,
            register_thread_launch,
            release_thread_runtime,
            register_address_space,
            release_address_space,
            map_memory_object,
            unmap_memory_range,
            update_memory_protection,
            translate_address,
        });
    }

    pub fn prepare_address_space(
        image: &[u8],
    ) -> Result<PreparedUserAddressSpace, AddressSpacePreparationError> {
        let memory = memory::manager().ok_or(AddressSpacePreparationError::NotInitialized)?;
        let mut frame_allocator = memory.frame_allocator().lock();
        let mut user_page_table = unsafe {
            OwnedPageTable::new_user_space(
                memory.kernel_address_space().root.level_4_frame,
                &mut frame_allocator,
            )
        }?;
        let loaded = serviceos_kernel_core::user::load_image(
            image,
            &mut user_page_table,
            &mut frame_allocator,
            ElfMachine::Aarch64,
            VirtualAddress::new(0x0000_7fff_ffff_0000),
        )?;

        Ok(PreparedUserAddressSpace {
            page_table_root: user_page_table.root_frame(),
            image: loaded,
        })
    }

    pub fn register_thread_launch(launch: UserThreadLaunch) {
        USER_THREADS
            .lock()
            .register_launch(launch)
            .expect("user thread runtime capacity must cover bootstrap services");
    }

    pub fn release_thread_runtime(thread_id: ThreadId) {
        USER_THREADS.lock().release_thread(thread_id);
    }

    pub fn register_address_space(
        address_space_id: AddressSpaceId,
        page_table_root: PhysicalAddress,
    ) {
        ADDRESS_SPACES
            .lock()
            .register(address_space_id, page_table_root);
    }

    pub fn release_address_space(address_space_id: AddressSpaceId) {
        ADDRESS_SPACES.lock().release(address_space_id);
    }

    pub fn unmap_memory_range(
        address_space_id: AddressSpaceId,
        virtual_start: VirtualAddress,
        page_count: usize,
    ) -> Result<(), MappingError> {
        let Some(root_frame) = ADDRESS_SPACES.lock().root(address_space_id) else {
            return Err(MappingError::Unsupported);
        };
        let Some(_memory) = memory::manager() else {
            return Err(MappingError::FrameAllocationFailed);
        };
        let mut mapper = unsafe { OwnedPageTable::from_root(root_frame) };
        for i in 0..page_count {
            let page_addr = virtual_start.offset((i as u64) * 4096);
            mapper.unmap_page(page_addr)?;
        }
        Ok(())
    }

    pub fn update_memory_protection(
        address_space_id: AddressSpaceId,
        virtual_start: VirtualAddress,
        page_count: usize,
        flags: MappingFlags,
    ) -> Result<(), MappingError> {
        let Some(root_frame) = ADDRESS_SPACES.lock().root(address_space_id) else {
            return Err(MappingError::Unsupported);
        };
        let Some(_memory) = memory::manager() else {
            return Err(MappingError::FrameAllocationFailed);
        };
        let mut mapper = unsafe { OwnedPageTable::from_root(root_frame) };
        let mut page_flags = MappingFlags::USER_ACCESSIBLE;
        if flags.contains(MappingFlags::WRITABLE) {
            page_flags |= MappingFlags::WRITABLE;
        }
        if flags.contains(MappingFlags::EXECUTABLE) {
            page_flags |= MappingFlags::EXECUTABLE;
        }
        for i in 0..page_count {
            let page_addr = virtual_start.offset((i as u64) * 4096);
            mapper.update_protection(page_addr, page_flags)?;
        }
        Ok(())
    }

    pub fn translate_address(
        address_space_id: AddressSpaceId,
        virtual_address: VirtualAddress,
    ) -> Option<PhysicalAddress> {
        let root_frame = ADDRESS_SPACES.lock().root(address_space_id)?;
        let mapper = unsafe { OwnedPageTable::from_root(root_frame) };
        mapper.translate(virtual_address)
    }

    pub fn map_memory_object(
        address_space_id: AddressSpaceId,
        virtual_start: VirtualAddress,
        frames: &[PhysicalAddress],
        writable: bool,
    ) -> Result<(), MappingError> {
        let Some(root_frame) = ADDRESS_SPACES.lock().root(address_space_id) else {
            return Err(MappingError::Unsupported);
        };
        let Some(memory) = memory::manager() else {
            return Err(MappingError::FrameAllocationFailed);
        };
        let mut allocator = memory.frame_allocator().lock();
        let mut mapper = unsafe { OwnedPageTable::from_root(root_frame) };
        let mut flags = MappingFlags::USER_ACCESSIBLE;
        if writable {
            flags |= MappingFlags::WRITABLE;
        }
        for (index, frame_base) in frames.iter().copied().enumerate() {
            mapper.map_page(
                virtual_start.offset((index as u64) * 4096),
                Frame {
                    base: frame_base,
                    size: PageSize::Size4KiB,
                },
                flags,
                &mut allocator,
            )?;
        }
        Ok(())
    }

    /// Snapshot a preempted user thread's context into its runtime slot so a
    /// later run_thread() resumes it from this exact state (x86 mirror of
    /// arch/x86_64/src/interrupts/irq.rs save_thread_context). Called from
    /// the timer IRQ handler only; clears resume_publish because an IRQ
    /// preemption is not a scheduler suspension — the thread's raw_syscall
    /// result slot, if a result is pending, must stay untouched.
    pub fn save_thread_context(thread_id: ThreadId, context: &SavedUserContext) -> bool {
        let mut snapshot = *context;
        snapshot.resume_publish = 0;
        let mut runtime = USER_THREADS.lock();
        match runtime.thread_mut(thread_id) {
            Some(thread) => {
                thread.context = snapshot;
                true
            }
            None => false,
        }
    }

    pub fn run_thread(thread_id: ThreadId) -> Result<(), UserLaunchError> {
        let (page_table_root, context_ptr) = {
            let mut runtime = USER_THREADS.lock();
            let Some(thread) = runtime.thread_mut(thread_id) else {
                return Err(UserLaunchError::UnknownThread);
            };
            (
                thread.page_table_root,
                &thread.context as *const SavedUserContext,
            )
        };

        let kernel_page_table_root = current_page_table_root();
        unsafe {
            load_page_table_root(page_table_root);
            serviceos_aarch64_resume_user(context_ptr);
            load_page_table_root(kernel_page_table_root);
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    use serviceos_kernel_core::{
        memory::{MappingError, MappingFlags, PhysicalAddress, VirtualAddress},
        task::{AddressSpaceId, ThreadId},
        user::{
            self, AddressSpacePreparationError, PreparedUserAddressSpace, UserArchHooks,
            UserThreadLaunch,
        },
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct SavedUserContext;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum UserLaunchError {
        Unsupported,
        Mapping(MappingError),
    }

    pub fn initialize() {
        user::register_arch_hooks(UserArchHooks {
            prepare_address_space,
            register_thread_launch,
            release_thread_runtime,
            register_address_space,
            release_address_space,
            map_memory_object,
            unmap_memory_range,
            update_memory_protection,
            translate_address,
        });
    }

    pub fn prepare_address_space(
        _image: &[u8],
    ) -> Result<PreparedUserAddressSpace, AddressSpacePreparationError> {
        Err(AddressSpacePreparationError::NotInitialized)
    }

    pub fn register_thread_launch(_launch: UserThreadLaunch) {}

    pub fn release_thread_runtime(_thread_id: ThreadId) {}

    pub fn register_address_space(
        _address_space_id: AddressSpaceId,
        _page_table_root: PhysicalAddress,
    ) {
    }

    pub fn release_address_space(_address_space_id: AddressSpaceId) {}

    pub fn map_memory_object(
        _address_space_id: AddressSpaceId,
        _virtual_start: VirtualAddress,
        _frames: &[PhysicalAddress],
        _writable: bool,
    ) -> Result<(), MappingError> {
        Err(MappingError::Unsupported)
    }

    pub fn unmap_memory_range(
        _address_space_id: AddressSpaceId,
        _virtual_start: VirtualAddress,
        _page_count: usize,
    ) -> Result<(), MappingError> {
        Err(MappingError::Unsupported)
    }

    pub fn update_memory_protection(
        _address_space_id: AddressSpaceId,
        _virtual_start: VirtualAddress,
        _page_count: usize,
        _flags: MappingFlags,
    ) -> Result<(), MappingError> {
        Err(MappingError::Unsupported)
    }

    pub fn translate_address(
        _address_space_id: AddressSpaceId,
        _virtual_address: VirtualAddress,
    ) -> Option<PhysicalAddress> {
        None
    }

    pub fn run_thread(_thread_id: ThreadId) -> Result<(), UserLaunchError> {
        Err(UserLaunchError::Unsupported)
    }

    pub fn save_thread_context(_thread_id: ThreadId, _context: &SavedUserContext) -> bool {
        false
    }
}

pub use imp::{
    SavedUserContext, UserLaunchError, initialize, map_memory_object, prepare_address_space,
    register_address_space, register_thread_launch, release_address_space, release_thread_runtime,
    run_thread, save_thread_context, translate_address, unmap_memory_range,
    update_memory_protection,
};
