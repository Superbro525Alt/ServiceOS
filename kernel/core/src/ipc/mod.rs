use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use spin::{Mutex, Once};

use crate::{
    capability::{
        CapabilityError, CapabilityHandle, CapabilityRights, CapabilitySpace, PreparedTransfer,
    },
    object::{KernelObjectModel, KernelObjectRef, KernelObjectWeak, ObjectId},
};

pub const MAX_MESSAGE_WORDS: usize = 16;
pub const MAX_MESSAGE_CAPABILITIES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct EndpointId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageTag(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageBufferDescriptor {
    pub word_count: usize,
    pub transfers_capability: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedMemoryHint {
    pub offset_bytes: usize,
    pub length_bytes: usize,
    pub writable: bool,
}

#[derive(Clone)]
pub struct OutgoingMessage {
    tag: MessageTag,
    words: Vec<u64>,
    capabilities: Vec<PreparedTransfer>,
    reply_endpoint: Option<PreparedTransfer>,
    shared_memory_hint: Option<SharedMemoryHint>,
}

impl OutgoingMessage {
    pub fn new(tag: MessageTag, words: &[u64]) -> Result<Self, IpcError> {
        if words.len() > MAX_MESSAGE_WORDS {
            return Err(IpcError::MessageTooLarge {
                word_count: words.len(),
                max_words: MAX_MESSAGE_WORDS,
            });
        }

        Ok(Self {
            tag,
            words: Vec::from(words),
            capabilities: Vec::new(),
            reply_endpoint: None,
            shared_memory_hint: None,
        })
    }

    pub fn add_transfer(mut self, transfer: PreparedTransfer) -> Result<Self, IpcError> {
        if self.capabilities.len() == MAX_MESSAGE_CAPABILITIES {
            return Err(IpcError::TooManyTransfers {
                transfer_count: self.capabilities.len() + 1,
                max_transfers: MAX_MESSAGE_CAPABILITIES,
            });
        }

        self.capabilities.push(transfer);
        Ok(self)
    }

    pub fn with_reply_endpoint(mut self, reply_endpoint: PreparedTransfer) -> Self {
        self.reply_endpoint = Some(reply_endpoint);
        self
    }

    pub fn with_shared_memory_hint(mut self, hint: SharedMemoryHint) -> Self {
        self.shared_memory_hint = Some(hint);
        self
    }

    pub fn descriptor(&self) -> MessageBufferDescriptor {
        MessageBufferDescriptor {
            word_count: self.words.len(),
            transfers_capability: !self.capabilities.is_empty() || self.reply_endpoint.is_some(),
        }
    }
}

#[derive(Clone)]
struct MessageEnvelope {
    tag: MessageTag,
    words: Vec<u64>,
    capabilities: Vec<PreparedTransfer>,
    reply_endpoint: Option<PreparedTransfer>,
    shared_memory_hint: Option<SharedMemoryHint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedMessage {
    pub tag: MessageTag,
    pub words: Vec<u64>,
    pub transferred_capabilities: Vec<CapabilityHandle>,
    pub reply_endpoint: Option<CapabilityHandle>,
    pub shared_memory_hint: Option<SharedMemoryHint>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageReceipt {
    pub peer: ObjectId,
    pub descriptor: MessageBufferDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelQueueState {
    pub peer: Option<ObjectId>,
    pub queued_messages: usize,
}

pub struct ChannelEndpointObject {
    state: Mutex<ChannelEndpointState>,
}

struct ChannelEndpointState {
    peer: KernelObjectWeak,
    queue: VecDeque<MessageEnvelope>,
}

impl ChannelEndpointObject {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ChannelEndpointState {
                peer: Weak::new(),
                queue: VecDeque::new(),
            }),
        }
    }

    pub fn connect(&self, peer: &KernelObjectRef) {
        self.state.lock().peer = Arc::downgrade(peer);
    }

    pub fn snapshot(&self) -> ChannelQueueState {
        let state = self.state.lock();

        ChannelQueueState {
            peer: state.peer.upgrade().map(|object| object.id()),
            queued_messages: state.queue.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcError {
    EndpointNotReady,
    BufferShapeInvalid,
    Capability(CapabilityError),
    ObjectKindMismatch,
    EndpointClosed,
    QueueEmpty,
    MessageTooLarge {
        word_count: usize,
        max_words: usize,
    },
    TooManyTransfers {
        transfer_count: usize,
        max_transfers: usize,
    },
}

impl From<CapabilityError> for IpcError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

pub trait IpcTransport {
    fn send(
        &self,
        endpoint: EndpointId,
        tag: MessageTag,
        buffer: MessageBufferDescriptor,
    ) -> Result<(), IpcError>;
}

pub struct IpcKernel;

impl IpcKernel {
    fn new() -> Self {
        Self
    }

    pub fn create_channel_pair(
        &self,
        objects: &KernelObjectModel,
    ) -> (KernelObjectRef, KernelObjectRef) {
        objects.registry().create_channel_pair()
    }

    pub fn send(
        &self,
        sender_space: &CapabilitySpace,
        endpoint_handle: CapabilityHandle,
        message: OutgoingMessage,
    ) -> Result<MessageReceipt, IpcError> {
        let descriptor = message.descriptor();
        let endpoint = sender_space.resolve(endpoint_handle, CapabilityRights::SEND)?;
        let Some(channel) = endpoint.object.channel_endpoint() else {
            return Err(IpcError::ObjectKindMismatch);
        };
        let peer = channel
            .state
            .lock()
            .peer
            .upgrade()
            .ok_or(IpcError::EndpointClosed)?;
        let Some(peer_channel) = peer.channel_endpoint() else {
            return Err(IpcError::ObjectKindMismatch);
        };

        peer_channel.state.lock().queue.push_back(MessageEnvelope {
            tag: message.tag,
            words: message.words,
            capabilities: message.capabilities,
            reply_endpoint: message.reply_endpoint,
            shared_memory_hint: message.shared_memory_hint,
        });

        Ok(MessageReceipt {
            peer: peer.id(),
            descriptor,
        })
    }

    pub fn receive(
        &self,
        receiver_space: &CapabilitySpace,
        endpoint_handle: CapabilityHandle,
    ) -> Result<ReceivedMessage, IpcError> {
        let endpoint = receiver_space.resolve(endpoint_handle, CapabilityRights::RECEIVE)?;
        let Some(channel) = endpoint.object.channel_endpoint() else {
            return Err(IpcError::ObjectKindMismatch);
        };
        let Some(message) = channel.state.lock().queue.pop_front() else {
            return Err(IpcError::QueueEmpty);
        };

        let transferred_capabilities = message
            .capabilities
            .into_iter()
            .map(|transfer| receiver_space.accept_transfer(transfer))
            .collect();
        let reply_endpoint = message
            .reply_endpoint
            .map(|transfer| receiver_space.accept_transfer(transfer));

        Ok(ReceivedMessage {
            tag: message.tag,
            words: message.words,
            transferred_capabilities,
            reply_endpoint,
            shared_memory_hint: message.shared_memory_hint,
        })
    }
}

static IPC_KERNEL: Once<IpcKernel> = Once::new();

pub fn initialize() -> &'static IpcKernel {
    IPC_KERNEL.call_once(IpcKernel::new)
}

pub fn kernel() -> Option<&'static IpcKernel> {
    IPC_KERNEL.get()
}
