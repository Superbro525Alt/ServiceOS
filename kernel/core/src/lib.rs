#![no_std]

extern crate alloc;

pub mod bootstrap;
pub mod capability;
pub mod display;
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

use bootstrap::{BootContext, BootstrapPlan};
use interrupts::InterruptState;
use ipc::IpcKernel;
use memory::{MemoryManager, PageMapper};
use object::KernelObjectModel;
use syscall::DispatchTable;
use task::TaskSystem;
use time::TimeManager;

/// Architecture-neutral kernel state constructed after early boot handoff
/// normalization. Real subsystem initialization starts in later phases.
pub struct Kernel<'boot> {
    boot_context: &'boot BootContext<'boot>,
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
        boot_context: &'boot BootContext<'boot>,
        mapper: &mut impl PageMapper,
        timer_tick_hz: u64,
    ) -> Result<Self, KernelInitError> {
        let memory = memory::initialize(boot_context, mapper)?;
        let interrupts = interrupts::initialize();
        let _ = input::initialize();
        let syscalls = syscall::initialize();
        let time = time::initialize(time::TimerSourceInfo {
            tick_hz: timer_tick_hz,
        })?;
        let objects = object::initialize();
        let ipc = ipc::initialize();
        let tasks = task::initialize(objects);

        Ok(Self {
            boot_context,
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

    pub fn boot_context(&self) -> &'boot BootContext<'boot> {
        self.boot_context
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
