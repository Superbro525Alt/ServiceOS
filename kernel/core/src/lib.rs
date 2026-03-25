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

/// Architecture-neutral kernel state constructed after early boot handoff
/// normalization. Real subsystem initialization starts in later phases.
pub struct Kernel<'boot> {
    boot_context: &'boot BootContext<'boot>,
    bootstrap_plan: BootstrapPlan,
}

impl<'boot> Kernel<'boot> {
    pub fn initialize(boot_context: &'boot BootContext<'boot>) -> Self {
        Self {
            boot_context,
            bootstrap_plan: BootstrapPlan::phase0(),
        }
    }

    pub fn boot_context(&self) -> &'boot BootContext<'boot> {
        self.boot_context
    }

    pub fn bootstrap_plan(&self) -> BootstrapPlan {
        self.bootstrap_plan
    }
}
