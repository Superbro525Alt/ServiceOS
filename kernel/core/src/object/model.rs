use alloc::sync::Arc;
use spin::Once;

use crate::capability::CapabilityRights;

use super::{KernelObjectRef, ObjectRegistry};

pub struct KernelObjectModel {
    registry: ObjectRegistry,
    bootstrap_task: KernelObjectRef,
    bootstrap_capability: KernelObjectRef,
}

impl KernelObjectModel {
    fn new() -> Self {
        let registry = ObjectRegistry::new();
        let bootstrap_task = registry.create_bootstrap_root_task();
        let bootstrap_capability = registry.create_bootstrap_capability();
        bootstrap_task
            .task()
            .expect("bootstrap task object")
            .capability_space()
            .install(
                Arc::clone(&bootstrap_task),
                CapabilityRights::task(),
                Some(0),
            )
            .expect("bootstrap task install must not exhaust the capability space");

        Self {
            registry,
            bootstrap_task,
            bootstrap_capability,
        }
    }

    pub fn registry(&self) -> &ObjectRegistry {
        &self.registry
    }

    pub fn bootstrap_task(&self) -> &KernelObjectRef {
        &self.bootstrap_task
    }

    pub fn bootstrap_capability(&self) -> &KernelObjectRef {
        &self.bootstrap_capability
    }
}

static OBJECT_MODEL: Once<KernelObjectModel> = Once::new();

pub fn initialize() -> &'static KernelObjectModel {
    OBJECT_MODEL.call_once(KernelObjectModel::new)
}

pub fn model() -> Option<&'static KernelObjectModel> {
    OBJECT_MODEL.get()
}
