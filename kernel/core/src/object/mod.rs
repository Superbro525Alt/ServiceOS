mod model;
mod objects;
mod registry;
mod types;

pub use model::{KernelObjectModel, initialize, model};
pub use objects::{
    BootstrapCapabilityObject, DmaSafety, EventObject, EventStateView, MemoryAccessError,
    MemoryObject, MemoryObjectInfo, PIPE_BUFFER_BYTES, PipeObject, PipeReadOutcome, PipeSnapshot,
    PipeWriteOutcome, TimerObject, TimerStateView,
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
    fn registry_collects_stale_weak_entries_during_runtime_registration() {
        let registry = ObjectRegistry::new();
        for _ in 0..ObjectRegistry::GC_CREATE_INTERVAL {
            let event = registry.create_event(false);
            drop(event);
        }

        let fresh = registry.create_event(false);
        let snapshot = registry.snapshot();

        assert!(snapshot.tracked_objects <= 2);
        assert!(registry.lookup(fresh.id()).is_some());
    }

    #[test]
    fn memory_object_reports_page_count() {
        let memory = MemoryObject::new(8193, true, DmaSafety::Unsafe);

        assert_eq!(
            memory.info(),
            MemoryObjectInfo {
                size_bytes: 8193,
                page_count: 3,
                writable: true,
                dma_safety: DmaSafety::Unsafe,
            }
        );
    }

    #[test]
    fn writable_memory_object_round_trips_bytes() {
        let memory = MemoryObject::new(16, true, DmaSafety::Unsafe);
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

    #[test]
    fn dma_safety_is_set_at_creation_and_immutable() {
        assert_eq!(DmaSafety::default(), DmaSafety::Unsafe);

        let unsafe_object = MemoryObject::new(4096, true, DmaSafety::Unsafe);
        let pinned = MemoryObject::new(4096, true, DmaSafety::PagePinned);
        let contiguous = MemoryObject::new(8192, true, DmaSafety::Contiguous);
        let from_bytes = MemoryObject::from_bytes(b"payload");

        assert_eq!(unsafe_object.info().dma_safety, DmaSafety::Unsafe);
        assert_eq!(pinned.info().dma_safety, DmaSafety::PagePinned);
        assert_eq!(contiguous.info().dma_safety, DmaSafety::Contiguous);
        assert_eq!(from_bytes.info().dma_safety, DmaSafety::Unsafe);

        // Classification is kernel-internal and immutable: writes to the
        // object's bytes never touch the class.
        assert_eq!(contiguous.write(0, b"x"), Ok(1));
        assert_eq!(contiguous.info().dma_safety, DmaSafety::Contiguous);
    }

    #[test]
    fn device_backing_rejects_unsafe_objects() {
        let object = MemoryObject::new(4096, true, DmaSafety::Unsafe);

        // The gate must fire before any physical surface is produced: the
        // error is the policy violation, not a resource shortage.
        assert_eq!(
            object.device_backing(),
            Err(MemoryAccessError::DmaPolicyViolation)
        );
    }

    #[test]
    fn device_backing_admits_page_pinned_objects() {
        let object = MemoryObject::new(4096, true, DmaSafety::PagePinned);

        // Host tests cannot materialize frames (no memory manager), so the
        // expected outcome is Busy: the gate admitted the object and it got
        // as far as frame allocation. Anything but DmaPolicyViolation means
        // the policy gate passed.
        assert_eq!(object.device_backing(), Err(MemoryAccessError::Busy));
    }

    #[test]
    fn frames_are_contiguous_accepts_runs_and_rejects_gaps() {
        use super::objects::frames_are_contiguous;
        use crate::memory::PhysicalAddress;

        let run = [
            PhysicalAddress::new(0x1000),
            PhysicalAddress::new(0x2000),
            PhysicalAddress::new(0x3000),
        ];
        assert!(frames_are_contiguous(&run));

        let gapped = [PhysicalAddress::new(0x1000), PhysicalAddress::new(0x3000)];
        assert!(!frames_are_contiguous(&gapped));

        let single = [PhysicalAddress::new(0x7000)];
        assert!(frames_are_contiguous(&single));
        assert!(frames_are_contiguous(&[]));
    }

    #[test]
    fn registry_roundtrip_preserves_dma_safety() {
        let registry = ObjectRegistry::new();

        let pinned = registry.create_memory_object(4096, true, DmaSafety::PagePinned);
        let pinned = pinned.memory_object().expect("memory object record");
        assert_eq!(pinned.info().dma_safety, DmaSafety::PagePinned);

        let contiguous = registry.create_memory_object(4096, true, DmaSafety::Contiguous);
        let contiguous = contiguous.memory_object().expect("memory object record");
        assert_eq!(contiguous.info().dma_safety, DmaSafety::Contiguous);

        let from_bytes = registry.create_memory_object_from_bytes(b"seed");
        let from_bytes = from_bytes.memory_object().expect("memory object record");
        assert_eq!(from_bytes.info().dma_safety, DmaSafety::Unsafe);
    }
}
