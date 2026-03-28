#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapStage {
    EarlyArchitectureBringUp,
    MemoryDiscovery,
    ControlFlowFoundation,
    KernelObjectFoundation,
    RootTaskPreparation,
    SchedulerFoundation,
    UserspaceBootstrap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapPlan {
    pub current: BootstrapStage,
    pub next: Option<BootstrapStage>,
}

impl BootstrapPlan {
    pub const fn architecture_bring_up() -> Self {
        Self {
            current: BootstrapStage::EarlyArchitectureBringUp,
            next: Some(BootstrapStage::MemoryDiscovery),
        }
    }

    pub const fn memory_ready() -> Self {
        Self {
            current: BootstrapStage::MemoryDiscovery,
            next: Some(BootstrapStage::ControlFlowFoundation),
        }
    }

    pub const fn control_flow_ready() -> Self {
        Self {
            current: BootstrapStage::ControlFlowFoundation,
            next: Some(BootstrapStage::KernelObjectFoundation),
        }
    }

    pub const fn object_foundation_ready() -> Self {
        Self {
            current: BootstrapStage::KernelObjectFoundation,
            next: Some(BootstrapStage::RootTaskPreparation),
        }
    }

    pub const fn root_task_ready() -> Self {
        Self {
            current: BootstrapStage::RootTaskPreparation,
            next: Some(BootstrapStage::SchedulerFoundation),
        }
    }

    pub const fn scheduler_ready() -> Self {
        Self {
            current: BootstrapStage::SchedulerFoundation,
            next: Some(BootstrapStage::UserspaceBootstrap),
        }
    }

    pub const fn userspace_bootstrap_ready() -> Self {
        Self {
            current: BootstrapStage::UserspaceBootstrap,
            next: None,
        }
    }
}
