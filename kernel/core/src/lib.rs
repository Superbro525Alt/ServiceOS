#![no_std]

pub mod bootstrap;
pub mod capability;
pub mod interrupts;
pub mod ipc;
pub mod memory;
pub mod object;
pub mod syscall;
pub mod task;
pub mod time;

use bootstrap::{BootContext, BootstrapPlan};
use memory::{MemoryManager, PageMapper};

/// Architecture-neutral kernel state constructed after early boot handoff
/// normalization. Real subsystem initialization starts in later phases.
pub struct Kernel<'boot> {
    boot_context: &'boot BootContext<'boot>,
    bootstrap_plan: BootstrapPlan,
    memory: &'static MemoryManager,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelInitError {
    Memory(memory::InitializationError),
}

impl From<memory::InitializationError> for KernelInitError {
    fn from(error: memory::InitializationError) -> Self {
        Self::Memory(error)
    }
}

impl<'boot> Kernel<'boot> {
    pub fn initialize(
        boot_context: &'boot BootContext<'boot>,
        mapper: &mut impl PageMapper,
    ) -> Result<Self, KernelInitError> {
        let memory = memory::initialize(boot_context, mapper)?;

        Ok(Self {
            boot_context,
            bootstrap_plan: BootstrapPlan::phase1(),
            memory,
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
}
