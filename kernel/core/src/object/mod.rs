use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use spin::{Mutex, Once};

use crate::{
    capability::CapabilityRights,
    ipc::ChannelEndpointObject,
    task::{
        TaskDescriptor, TaskId, TaskObject, TaskRole, ThreadDescriptor, ThreadId, ThreadObject,
    },
    time::MonotonicInstant,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ObjectId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Task,
    Thread,
    ChannelEndpoint,
    Event,
    Timer,
    MemoryObject,
    BootstrapCapability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectHeader {
    pub id: ObjectId,
    pub kind: ObjectKind,
}

pub type KernelObjectRef = Arc<KernelObjectRecord>;
pub type KernelObjectWeak = Weak<KernelObjectRecord>;

pub struct KernelObjectRecord {
    header: ObjectHeader,
    body: KernelObject,
}

pub enum KernelObject {
    Task(TaskObject),
    Thread(ThreadObject),
    ChannelEndpoint(ChannelEndpointObject),
    Event(EventObject),
    Timer(TimerObject),
    MemoryObject(MemoryObject),
    BootstrapCapability(BootstrapCapabilityObject),
}

impl KernelObjectRecord {
    pub const fn header(&self) -> ObjectHeader {
        self.header
    }

    pub const fn id(&self) -> ObjectId {
        self.header.id
    }

    pub const fn kind(&self) -> ObjectKind {
        self.header.kind
    }

    pub fn task(&self) -> Option<&TaskObject> {
        match &self.body {
            KernelObject::Task(task) => Some(task),
            _ => None,
        }
    }

    pub fn thread(&self) -> Option<&ThreadObject> {
        match &self.body {
            KernelObject::Thread(thread) => Some(thread),
            _ => None,
        }
    }

    pub fn channel_endpoint(&self) -> Option<&ChannelEndpointObject> {
        match &self.body {
            KernelObject::ChannelEndpoint(endpoint) => Some(endpoint),
            _ => None,
        }
    }

    pub fn event(&self) -> Option<&EventObject> {
        match &self.body {
            KernelObject::Event(event) => Some(event),
            _ => None,
        }
    }

    pub fn timer(&self) -> Option<&TimerObject> {
        match &self.body {
            KernelObject::Timer(timer) => Some(timer),
            _ => None,
        }
    }

    pub fn memory_object(&self) -> Option<&MemoryObject> {
        match &self.body {
            KernelObject::MemoryObject(memory) => Some(memory),
            _ => None,
        }
    }

    pub fn bootstrap_capability(&self) -> Option<&BootstrapCapabilityObject> {
        match &self.body {
            KernelObject::BootstrapCapability(authority) => Some(authority),
            _ => None,
        }
    }
}

pub struct BootstrapCapabilityObject;

impl BootstrapCapabilityObject {
    pub const fn new() -> Self {
        Self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventStateView {
    pub signaled: bool,
    pub signal_count: u64,
}

pub struct EventObject {
    state: Mutex<EventState>,
}

struct EventState {
    signaled: bool,
    signal_count: u64,
}

impl EventObject {
    pub fn new(signaled: bool) -> Self {
        Self {
            state: Mutex::new(EventState {
                signaled,
                signal_count: 0,
            }),
        }
    }

    pub fn signal(&self) {
        let mut state = self.state.lock();
        state.signaled = true;
        state.signal_count = state.signal_count.saturating_add(1);
    }

    pub fn reset(&self) {
        self.state.lock().signaled = false;
    }

    pub fn snapshot(&self) -> EventStateView {
        let state = self.state.lock();

        EventStateView {
            signaled: state.signaled,
            signal_count: state.signal_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerStateView {
    pub armed: bool,
    pub deadline: Option<MonotonicInstant>,
    pub periodic_interval_ticks: Option<u64>,
}

pub struct TimerObject {
    state: Mutex<TimerState>,
}

struct TimerState {
    armed: bool,
    deadline: Option<MonotonicInstant>,
    periodic_interval_ticks: Option<u64>,
}

impl TimerObject {
    pub fn new(deadline: Option<MonotonicInstant>, periodic_interval_ticks: Option<u64>) -> Self {
        Self {
            state: Mutex::new(TimerState {
                armed: deadline.is_some(),
                deadline,
                periodic_interval_ticks,
            }),
        }
    }

    pub fn arm(&self, deadline: MonotonicInstant, periodic_interval_ticks: Option<u64>) {
        let mut state = self.state.lock();
        state.armed = true;
        state.deadline = Some(deadline);
        state.periodic_interval_ticks = periodic_interval_ticks;
    }

    pub fn disarm(&self) {
        let mut state = self.state.lock();
        state.armed = false;
        state.deadline = None;
        state.periodic_interval_ticks = None;
    }

    pub fn snapshot(&self) -> TimerStateView {
        let state = self.state.lock();

        TimerStateView {
            armed: state.armed,
            deadline: state.deadline,
            periodic_interval_ticks: state.periodic_interval_ticks,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryObjectInfo {
    pub size_bytes: usize,
    pub page_count: usize,
    pub writable: bool,
}

pub struct MemoryObject {
    info: MemoryObjectInfo,
    bytes: Option<Arc<[u8]>>,
}

impl MemoryObject {
    pub fn new(size_bytes: usize, writable: bool) -> Self {
        let page_count = size_bytes.div_ceil(4096);

        Self {
            info: MemoryObjectInfo {
                size_bytes,
                page_count,
                writable,
            },
            bytes: None,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let size_bytes = bytes.len();
        let page_count = size_bytes.div_ceil(4096);
        Self {
            info: MemoryObjectInfo {
                size_bytes,
                page_count,
                writable: false,
            },
            bytes: Some(Arc::from(bytes)),
        }
    }

    pub const fn info(&self) -> MemoryObjectInfo {
        self.info
    }

    pub fn read(&self, offset: usize, destination: &mut [u8]) -> usize {
        let Some(bytes) = &self.bytes else {
            return 0;
        };
        let Some(source) = bytes.get(offset..) else {
            return 0;
        };
        let len = source.len().min(destination.len());
        destination[..len].copy_from_slice(&source[..len]);
        len
    }
}

struct ObjectRegistryState {
    next_id: u64,
    live: BTreeMap<ObjectId, KernelObjectWeak>,
}

pub struct ObjectRegistry {
    state: Mutex<ObjectRegistryState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectRegistrySnapshot {
    pub next_id: u64,
    pub tracked_objects: usize,
}

impl ObjectRegistry {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ObjectRegistryState {
                next_id: 1,
                live: BTreeMap::new(),
            }),
        }
    }

    pub fn create_bootstrap_root_task(&self) -> KernelObjectRef {
        self.create_task(TaskDescriptor {
            address_space: None,
            role: TaskRole::BootstrapRoot,
        })
    }

    pub fn create_task(&self, descriptor: TaskDescriptor) -> KernelObjectRef {
        let id = self.allocate_id(ObjectKind::Task);
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id,
                kind: ObjectKind::Task,
            },
            body: KernelObject::Task(TaskObject::new(TaskId(id.0), descriptor)),
        })
    }

    pub fn create_thread(
        &self,
        owner_task: &KernelObjectRef,
        descriptor: ThreadDescriptor,
    ) -> KernelObjectRef {
        let owner = owner_task
            .task()
            .expect("thread owners must be task objects")
            .id();
        let id = self.allocate_id(ObjectKind::Thread);
        let thread = self.register(KernelObjectRecord {
            header: ObjectHeader {
                id,
                kind: ObjectKind::Thread,
            },
            body: KernelObject::Thread(ThreadObject::new(ThreadId(id.0), owner, descriptor)),
        });
        owner_task
            .task()
            .expect("thread owners must be task objects")
            .attach_thread(id);
        thread
    }

    pub fn create_channel_pair(&self) -> (KernelObjectRef, KernelObjectRef) {
        let first = self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::ChannelEndpoint),
                kind: ObjectKind::ChannelEndpoint,
            },
            body: KernelObject::ChannelEndpoint(ChannelEndpointObject::new()),
        });
        let second = self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::ChannelEndpoint),
                kind: ObjectKind::ChannelEndpoint,
            },
            body: KernelObject::ChannelEndpoint(ChannelEndpointObject::new()),
        });

        first
            .channel_endpoint()
            .expect("channel endpoint object")
            .connect(&second);
        second
            .channel_endpoint()
            .expect("channel endpoint object")
            .connect(&first);

        (first, second)
    }

    pub fn create_event(&self, signaled: bool) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::Event),
                kind: ObjectKind::Event,
            },
            body: KernelObject::Event(EventObject::new(signaled)),
        })
    }

    pub fn create_timer(
        &self,
        deadline: Option<MonotonicInstant>,
        periodic_interval_ticks: Option<u64>,
    ) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::Timer),
                kind: ObjectKind::Timer,
            },
            body: KernelObject::Timer(TimerObject::new(deadline, periodic_interval_ticks)),
        })
    }

    pub fn create_memory_object(&self, size_bytes: usize, writable: bool) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::MemoryObject),
                kind: ObjectKind::MemoryObject,
            },
            body: KernelObject::MemoryObject(MemoryObject::new(size_bytes, writable)),
        })
    }

    pub fn create_memory_object_from_bytes(&self, bytes: &[u8]) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::MemoryObject),
                kind: ObjectKind::MemoryObject,
            },
            body: KernelObject::MemoryObject(MemoryObject::from_bytes(bytes)),
        })
    }

    pub fn create_bootstrap_capability(&self) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::BootstrapCapability),
                kind: ObjectKind::BootstrapCapability,
            },
            body: KernelObject::BootstrapCapability(BootstrapCapabilityObject::new()),
        })
    }

    pub fn lookup(&self, id: ObjectId) -> Option<KernelObjectRef> {
        self.state.lock().live.get(&id).and_then(Weak::upgrade)
    }

    pub fn collect_garbage(&self) {
        self.state
            .lock()
            .live
            .retain(|_, object| object.strong_count() > 0);
    }

    pub fn snapshot(&self) -> ObjectRegistrySnapshot {
        let state = self.state.lock();

        ObjectRegistrySnapshot {
            next_id: state.next_id,
            tracked_objects: state.live.len(),
        }
    }

    fn allocate_id(&self, kind: ObjectKind) -> ObjectId {
        let mut state = self.state.lock();
        let id = ObjectId(state.next_id);
        state.next_id = state.next_id.saturating_add(1);
        let _ = kind;
        id
    }

    fn register(&self, object: KernelObjectRecord) -> KernelObjectRef {
        let object = Arc::new(object);
        self.state
            .lock()
            .live
            .insert(object.id(), Arc::downgrade(&object));
        object
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn bootstrap_capability_has_distinct_object_kind() {
        let registry = ObjectRegistry::new();
        let authority = registry.create_bootstrap_capability();

        assert_eq!(authority.kind(), ObjectKind::BootstrapCapability);
        assert!(authority.bootstrap_capability().is_some());
    }
}
