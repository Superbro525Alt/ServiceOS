use alloc::sync::{Arc, Weak};

use crate::{
    audio::AudioEndpointObject,
    block::BlockDeviceObject,
    display::DisplayOutputObject,
    input::InputSourceObject,
    ipc::ChannelEndpointObject,
    network::PacketInterfaceObject,
    task::{TaskObject, ThreadObject},
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
    PacketInterface,
    DisplayOutput,
    InputSource,
    AudioEndpoint,
    BlockDevice,
    Pipe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectHeader {
    pub id: ObjectId,
    pub kind: ObjectKind,
}

pub type KernelObjectRef = Arc<KernelObjectRecord>;
pub type KernelObjectWeak = Weak<KernelObjectRecord>;

pub struct KernelObjectRecord {
    pub(super) header: ObjectHeader,
    pub(super) body: KernelObject,
}

pub enum KernelObject {
    Task(TaskObject),
    Thread(ThreadObject),
    ChannelEndpoint(ChannelEndpointObject),
    Event(super::objects::EventObject),
    Timer(super::objects::TimerObject),
    MemoryObject(super::objects::MemoryObject),
    BootstrapCapability(super::objects::BootstrapCapabilityObject),
    PacketInterface(PacketInterfaceObject),
    DisplayOutput(DisplayOutputObject),
    InputSource(InputSourceObject),
    AudioEndpoint(AudioEndpointObject),
    BlockDevice(BlockDeviceObject),
    Pipe(super::objects::PipeObject),
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

    pub fn event(&self) -> Option<&super::objects::EventObject> {
        match &self.body {
            KernelObject::Event(event) => Some(event),
            _ => None,
        }
    }

    pub fn timer(&self) -> Option<&super::objects::TimerObject> {
        match &self.body {
            KernelObject::Timer(timer) => Some(timer),
            _ => None,
        }
    }

    pub fn memory_object(&self) -> Option<&super::objects::MemoryObject> {
        match &self.body {
            KernelObject::MemoryObject(memory) => Some(memory),
            _ => None,
        }
    }

    pub fn bootstrap_capability(&self) -> Option<&super::objects::BootstrapCapabilityObject> {
        match &self.body {
            KernelObject::BootstrapCapability(authority) => Some(authority),
            _ => None,
        }
    }

    pub fn packet_interface(&self) -> Option<&PacketInterfaceObject> {
        match &self.body {
            KernelObject::PacketInterface(interface) => Some(interface),
            _ => None,
        }
    }

    pub fn display_output(&self) -> Option<&DisplayOutputObject> {
        match &self.body {
            KernelObject::DisplayOutput(output) => Some(output),
            _ => None,
        }
    }

    pub fn input_source(&self) -> Option<&InputSourceObject> {
        match &self.body {
            KernelObject::InputSource(source) => Some(source),
            _ => None,
        }
    }

    pub fn audio_endpoint(&self) -> Option<&AudioEndpointObject> {
        match &self.body {
            KernelObject::AudioEndpoint(endpoint) => Some(endpoint),
            _ => None,
        }
    }

    pub fn block_device(&self) -> Option<&BlockDeviceObject> {
        match &self.body {
            KernelObject::BlockDevice(device) => Some(device),
            _ => None,
        }
    }

    pub fn pipe(&self) -> Option<&super::objects::PipeObject> {
        match &self.body {
            KernelObject::Pipe(pipe) => Some(pipe),
            _ => None,
        }
    }
}
