#![no_std]

extern crate alloc;

pub mod audio;
pub mod block;
pub mod bootstrap;
pub mod capability;
pub mod display;
pub mod fault;
pub mod input;
pub mod interrupts;
pub mod ipc;
pub mod memory;
pub mod network;
pub mod object;
pub mod syscall;
pub mod task;
pub mod time;
pub mod user;

use bootstrap::{BootInfo, BootstrapPlan};
use interrupts::InterruptState;
use ipc::IpcKernel;
use memory::{MemoryManager, PageMapper};
use object::KernelObjectModel;
use syscall::DispatchTable;
use task::TaskSystem;
use time::TimeManager;

/// TEMPORARY qemu-isa bring-up breadcrumb (REMOVE): raw byte to COM1.
#[doc(hidden)]
pub fn debug_probe(tag: u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "outb %al, %dx",
            in("al") tag,
            in("dx") 0x3f8u16,
            lateout("dx") _,
            options(att_syntax, nostack, preserves_flags)
        );
        core::arch::asm!(
            "outb %al, %dx",
            in("al") b'\n',
            in("dx") 0x3f8u16,
            lateout("dx") _,
            options(att_syntax, nostack, preserves_flags)
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = tag;
}

/// Architecture-neutral kernel state constructed after early boot handoff
/// normalization. Real subsystem initialization starts in later phases.
pub struct Kernel<'boot> {
    boot_info: &'boot BootInfo<'boot>,
    bootstrap_plan: BootstrapPlan,
    memory: &'static MemoryManager,
    interrupts: &'static InterruptState,
    syscalls: &'static DispatchTable,
    time: &'static TimeManager,
    objects: &'static KernelObjectModel,
    ipc: &'static IpcKernel,
    tasks: &'static TaskSystem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelInitError {
    Memory(memory::InitializationError),
    Time(time::InitializationError),
}

impl From<memory::InitializationError> for KernelInitError {
    fn from(error: memory::InitializationError) -> Self {
        Self::Memory(error)
    }
}

impl From<time::InitializationError> for KernelInitError {
    fn from(error: time::InitializationError) -> Self {
        Self::Time(error)
    }
}

impl<'boot> Kernel<'boot> {
    pub fn initialize(
        boot_info: &'boot BootInfo<'boot>,
        mapper: &mut impl PageMapper,
        timer_tick_hz: u64,
    ) -> Result<Self, KernelInitError> {
        debug_probe(b'1');
        let memory = memory::initialize(boot_info, mapper)?;
        debug_probe(b'2');
        let interrupts = interrupts::initialize();
        debug_probe(b'3');
        let _ = input::initialize();
        debug_probe(b'4');
        let syscalls = syscall::initialize();
        debug_probe(b'5');
        let time = time::initialize(time::TimerSourceInfo {
            tick_hz: timer_tick_hz,
        })?;
        debug_probe(b'6');
        let objects = object::initialize();
        debug_probe(b'7');
        let ipc = ipc::initialize();
        debug_probe(b'8');
        let tasks = task::initialize(objects);
        debug_probe(b'9');

        Ok(Self {
            boot_info,
            bootstrap_plan: BootstrapPlan::userspace_bootstrap_ready(),
            memory,
            interrupts,
            syscalls,
            time,
            objects,
            ipc,
            tasks,
        })
    }

    pub fn boot_info(&self) -> &'boot BootInfo<'boot> {
        self.boot_info
    }

    pub fn boot_context(&self) -> &'boot BootInfo<'boot> {
        self.boot_info
    }

    pub fn bootstrap_plan(&self) -> BootstrapPlan {
        self.bootstrap_plan
    }

    pub fn memory(&self) -> &'static MemoryManager {
        self.memory
    }

    pub fn interrupts(&self) -> &'static InterruptState {
        self.interrupts
    }

    pub fn syscalls(&self) -> &'static DispatchTable {
        self.syscalls
    }

    pub fn time(&self) -> &'static TimeManager {
        self.time
    }

    pub fn objects(&self) -> &'static KernelObjectModel {
        self.objects
    }

    pub fn ipc(&self) -> &'static IpcKernel {
        self.ipc
    }

    pub fn tasks(&self) -> &'static TaskSystem {
        self.tasks
    }
}
