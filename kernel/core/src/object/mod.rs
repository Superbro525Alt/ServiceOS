mod model;
mod objects;
mod registry;
mod types;

pub use model::{KernelObjectModel, initialize, model};
pub use objects::{
    BootstrapCapabilityObject, EventObject, EventStateView, MemoryAccessError, MemoryObject,
    MemoryObjectInfo, TimerObject, TimerStateView,
};
pub use registry::{ObjectRegistry, ObjectRegistrySnapshot};
pub use types::{
    KernelObject, KernelObjectRecord, KernelObjectRef, KernelObjectWeak, ObjectHeader, ObjectId,
    ObjectKind,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{TaskDescriptor, TaskId, TaskObject, TaskRole};

    #[test]
    fn task_attach_thread_is_idempotent() {
        let task = TaskObject::new(
            TaskId(1),
            TaskDescriptor {
                address_space: None,
                role: TaskRole::BootstrapRoot,
            },
        );

        task.attach_thread(ObjectId(9));
        task.attach_thread(ObjectId(9));

        assert_eq!(task.snapshot().thread_count, 1);
    }

    #[test]
    fn registry_collects_dropped_objects() {
        let registry = ObjectRegistry::new();
        let event = registry.create_event(false);
        let id = event.id();

        assert!(registry.lookup(id).is_some());
        drop(event);
        registry.collect_garbage();
        assert!(registry.lookup(id).is_none());
    }

    #[test]
    fn memory_object_reports_page_count() {
        let memory = MemoryObject::new(8193, true);

        assert_eq!(
            memory.info(),
            MemoryObjectInfo {
                size_bytes: 8193,
                page_count: 3,
                writable: true,
            }
        );
    }

    #[test]
    fn writable_memory_object_round_trips_bytes() {
        let memory = MemoryObject::new(16, true);
        assert_eq!(memory.write(4, b"abcd"), Ok(4));

        let mut bytes = [0u8; 8];
        assert_eq!(memory.read(0, &mut bytes), 8);
        assert_eq!(&bytes[4..8], b"abcd");
    }

    #[test]
    fn bootstrap_capability_has_distinct_object_kind() {
        let registry = ObjectRegistry::new();
        let authority = registry.create_bootstrap_capability();

        assert_eq!(authority.kind(), ObjectKind::BootstrapCapability);
        assert!(authority.bootstrap_capability().is_some());
    }
}
