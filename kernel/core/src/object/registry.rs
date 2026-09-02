use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use spin::Mutex;

use crate::{
    audio::{AudioBackend, AudioEndpointObject},
    block::{BlockBackend, BlockDeviceObject},
    display::{DisplayBackend, DisplayOutputObject},
    input::{self, InputBackend, InputSourceObject},
    ipc::ChannelEndpointObject,
    network::{self, PacketBackend, PacketInterfaceObject},
    task::{
        TaskDescriptor, TaskId, TaskIsolationClass, TaskObject, TaskRole, ThreadDescriptor,
        ThreadId, ThreadObject,
    },
    time::MonotonicInstant,
};

use super::{
    KernelObject, KernelObjectRecord, KernelObjectRef, KernelObjectWeak, ObjectHeader, ObjectId,
    ObjectKind,
    objects::{
        BootstrapCapabilityObject, DmaSafety, EventObject, MemoryObject, PipeObject, TimerObject,
    },
};

struct ObjectRegistryState {
    next_id: u64,
    live: BTreeMap<ObjectId, KernelObjectWeak>,
    creates_since_gc: usize,
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
    pub(crate) const GC_CREATE_INTERVAL: usize = 64;

    pub fn new() -> Self {
        Self {
            state: Mutex::new(ObjectRegistryState {
                next_id: 1,
                live: BTreeMap::new(),
                creates_since_gc: 0,
            }),
        }
    }

    pub fn create_bootstrap_root_task(&self) -> KernelObjectRef {
        self.create_task(TaskDescriptor {
            address_space: None,
            role: TaskRole::BootstrapRoot,
            isolation: TaskIsolationClass::Unrestricted,
            owner_env: None,
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

    pub fn create_memory_object(
        &self,
        size_bytes: usize,
        writable: bool,
        dma_safety: DmaSafety,
    ) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::MemoryObject),
                kind: ObjectKind::MemoryObject,
            },
            body: KernelObject::MemoryObject(MemoryObject::new(size_bytes, writable, dma_safety)),
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

    pub fn create_pipe(&self) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::Pipe),
                kind: ObjectKind::Pipe,
            },
            body: KernelObject::Pipe(PipeObject::new()),
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

    pub fn create_packet_interface(&self, backend: Arc<dyn PacketBackend>) -> KernelObjectRef {
        let object = self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::PacketInterface),
                kind: ObjectKind::PacketInterface,
            },
            body: KernelObject::PacketInterface(PacketInterfaceObject::new(backend)),
        });
        let packet = object
            .packet_interface()
            .expect("packet interface object must be a packet interface")
            .backend();
        let _ = network::initialize().register_interface(object.id().0, packet);
        object
    }

    pub fn create_display_output(&self, backend: Arc<dyn DisplayBackend>) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::DisplayOutput),
                kind: ObjectKind::DisplayOutput,
            },
            body: KernelObject::DisplayOutput(DisplayOutputObject::new(backend)),
        })
    }

    pub fn create_input_source(&self, backend: Arc<dyn InputBackend>) -> KernelObjectRef {
        let object = self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::InputSource),
                kind: ObjectKind::InputSource,
            },
            body: KernelObject::InputSource(InputSourceObject::new(backend)),
        });
        let source = object
            .input_source()
            .expect("input source object must be an input source")
            .backend();
        let _ = input::initialize().register_source(object.id().0, source);
        object
    }

    pub fn create_audio_endpoint(&self, backend: Arc<dyn AudioBackend>) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::AudioEndpoint),
                kind: ObjectKind::AudioEndpoint,
            },
            body: KernelObject::AudioEndpoint(AudioEndpointObject::new(backend)),
        })
    }

    pub fn create_block_device(&self, backend: Arc<dyn BlockBackend>) -> KernelObjectRef {
        self.register(KernelObjectRecord {
            header: ObjectHeader {
                id: self.allocate_id(ObjectKind::BlockDevice),
                kind: ObjectKind::BlockDevice,
            },
            body: KernelObject::BlockDevice(BlockDeviceObject::new(backend)),
        })
    }

    pub fn lookup(&self, id: ObjectId) -> Option<KernelObjectRef> {
        self.state.lock().live.get(&id).and_then(Weak::upgrade)
    }

    pub fn collect_garbage(&self) {
        collect_dead_objects(&mut self.state.lock());
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
        let mut state = self.state.lock();
        state.creates_since_gc = state.creates_since_gc.saturating_add(1);
        if state.creates_since_gc >= Self::GC_CREATE_INTERVAL {
            collect_dead_objects(&mut state);
        }
        state.live.insert(object.id(), Arc::downgrade(&object));
        object
    }
}

fn collect_dead_objects(state: &mut ObjectRegistryState) {
    state.live.retain(|_, object| object.strong_count() > 0);
    state.creates_since_gc = 0;
}
