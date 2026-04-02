use core::arch::global_asm;

use spin::Mutex;

use crate::{cpu, interrupts, paging::OwnedPageTable};
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

const MAX_USER_THREADS: usize = 64;
const MAX_USER_ADDRESS_SPACES: usize = 64;

global_asm!(
    r#"
.global serviceos_x86_64_resume_user
serviceos_x86_64_resume_user:
    push rbx
    push rbp
    push rdi
    push rsi
    push r12
    push r13
    push r14
    push r15
    mov [rip + serviceos_x86_64_user_return_stack], rsp
    mov r11, rcx
    push qword ptr [r11 + 0x98]
    push qword ptr [r11 + 0x90]
    push qword ptr [r11 + 0x88]
    push qword ptr [r11 + 0x80]
    push qword ptr [r11 + 0x78]
    mov r15, [r11 + 0x00]
    mov r14, [r11 + 0x08]
    mov r13, [r11 + 0x10]
    mov r12, [r11 + 0x18]
    mov r10, [r11 + 0x28]
    mov r9, [r11 + 0x30]
    mov r8, [r11 + 0x38]
    mov rdi, [r11 + 0x40]
    mov rsi, [r11 + 0x48]
    mov rbp, [r11 + 0x50]
    mov rdx, [r11 + 0x58]
    mov rcx, [r11 + 0x60]
    mov rbx, [r11 + 0x68]
    mov rax, [r11 + 0x70]
    mov r11, [r11 + 0x20]
    iretq

.global serviceos_x86_64_return_to_kernel
serviceos_x86_64_return_to_kernel:
    mov rsp, [rip + serviceos_x86_64_user_return_stack]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rsi
    pop rdi
    pop rbp
    pop rbx
    ret
"#
);

unsafe extern "C" {
    fn serviceos_x86_64_resume_user(context: *const SavedUserContext);
    fn serviceos_x86_64_return_to_kernel() -> !;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedUserContext {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub user_stack_pointer: u64,
    pub user_stack_segment: u64,
}

impl SavedUserContext {
    fn initial(entry_point: u64, user_stack_pointer: u64) -> Self {
        Self {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdi: 0,
            rsi: 0,
            rbp: 0,
            rdx: 0,
            rcx: 0,
            rbx: 0,
            rax: 0,
            instruction_pointer: entry_point,
            code_segment: interrupts::user_code_selector().0 as u64,
            cpu_flags: 0x202,
            user_stack_pointer,
            user_stack_segment: interrupts::user_data_selector().0 as u64,
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

    fn context_mut(&mut self, thread_id: ThreadId) -> Option<&mut SavedUserContext> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|thread| thread.thread_id == thread_id)
            .map(|thread| &mut thread.context)
    }

    fn thread(&self, thread_id: ThreadId) -> Option<SavedUserThread> {
        self.slots
            .iter()
            .flatten()
            .copied()
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
static mut serviceos_x86_64_user_return_stack: u64 = 0;

static USER_THREADS: Mutex<UserThreadRuntime> = Mutex::new(UserThreadRuntime::new());
static ADDRESS_SPACES: Mutex<AddressSpaceRuntime> = Mutex::new(AddressSpaceRuntime::new());

pub fn initialize() {
    user::register_arch_hooks(UserArchHooks {
        prepare_address_space,
        register_thread_launch,
        release_thread_runtime,
        register_address_space,
        release_address_space,
        map_memory_object,
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
        ElfMachine::X86_64,
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

pub fn register_address_space(address_space_id: AddressSpaceId, page_table_root: PhysicalAddress) {
    ADDRESS_SPACES
        .lock()
        .register(address_space_id, page_table_root);
}

pub fn release_address_space(address_space_id: AddressSpaceId) {
    ADDRESS_SPACES.lock().release(address_space_id);
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

pub fn save_thread_context(thread_id: ThreadId, context: &SavedUserContext) {
    if let Some(slot) = USER_THREADS.lock().context_mut(thread_id) {
        *slot = *context;
    }
}

pub fn run_thread(thread_id: ThreadId) -> Result<(), UserLaunchError> {
    let Some(thread) = USER_THREADS.lock().thread(thread_id) else {
        return Err(UserLaunchError::UnknownThread);
    };
    let kernel_page_table_root = cpu::current_page_table_root();

    unsafe {
        cpu::load_page_table_root(thread.page_table_root);
        serviceos_x86_64_resume_user(&thread.context);
        cpu::load_page_table_root(kernel_page_table_root);
    }

    Ok(())
}

pub fn return_to_kernel() -> ! {
    unsafe { serviceos_x86_64_return_to_kernel() }
}
