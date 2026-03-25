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
        }
    }

    pub const fn info(&self) -> MemoryObjectInfo {
        self.info
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
}

impl KernelObjectModel {
    fn new() -> Self {
        let registry = ObjectRegistry::new();
        let bootstrap_task = registry.create_bootstrap_root_task();
        bootstrap_task
            .task()
            .expect("bootstrap task object")
            .capability_space()
            .install(
                Arc::clone(&bootstrap_task),
                CapabilityRights::task(),
                Some(0),
            );

        Self {
            registry,
            bootstrap_task,
        }
    }

    pub fn registry(&self) -> &ObjectRegistry {
        &self.registry
    }

    pub fn bootstrap_task(&self) -> &KernelObjectRef {
        &self.bootstrap_task
    }
}

static OBJECT_MODEL: Once<KernelObjectModel> = Once::new();

pub fn initialize() -> &'static KernelObjectModel {
    OBJECT_MODEL.call_once(KernelObjectModel::new)
}

pub fn model() -> Option<&'static KernelObjectModel> {
    OBJECT_MODEL.get()
}
