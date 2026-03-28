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
pub const MAX_MESSAGE_CAPABILITIES: usize = serviceos_abi::IPC_MAX_HANDLES;
pub const MAX_QUEUED_MESSAGES_PER_ENDPOINT: usize = 64;

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
    InvalidReplyEndpoint,
    QueueEmpty,
    QueueFull {
        queued_messages: usize,
        max_messages: usize,
    },
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

        if let Some(reply_endpoint) = &message.reply_endpoint {
            if reply_endpoint.object().channel_endpoint().is_none() {
                return Err(IpcError::InvalidReplyEndpoint);
            }
        }

        let mut peer_state = peer_channel.state.lock();
        if peer_state.queue.len() >= MAX_QUEUED_MESSAGES_PER_ENDPOINT {
            return Err(IpcError::QueueFull {
                queued_messages: peer_state.queue.len(),
                max_messages: MAX_QUEUED_MESSAGES_PER_ENDPOINT,
            });
        }

        peer_state.queue.push_back(MessageEnvelope {
            tag: message.tag,
            words: message.words,
            capabilities: message.capabilities,
            reply_endpoint: message.reply_endpoint,
            shared_memory_hint: message.shared_memory_hint,
        });
        let _ = crate::task::notify_channel_ready(peer.id());

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
            .collect::<Result<Vec<_>, _>>()?;
        let reply_endpoint = message
            .reply_endpoint
            .map(|transfer| receiver_space.accept_transfer(transfer))
            .transpose()?;

        Ok(ReceivedMessage {
            tag: message.tag,
            words: message.words,
            transferred_capabilities,
            reply_endpoint,
            shared_memory_hint: message.shared_memory_hint,
        })
    }

    pub fn endpoint_object_id(
        &self,
        capability_space: &CapabilitySpace,
        endpoint_handle: CapabilityHandle,
        required: CapabilityRights,
    ) -> Result<ObjectId, IpcError> {
        let endpoint = capability_space.resolve(endpoint_handle, required)?;
        if endpoint.object.channel_endpoint().is_none() {
            return Err(IpcError::ObjectKindMismatch);
        }

        Ok(endpoint.object.id())
    }
}

static IPC_KERNEL: Once<IpcKernel> = Once::new();

pub fn initialize() -> &'static IpcKernel {
    IPC_KERNEL.call_once(IpcKernel::new)
}

pub fn kernel() -> Option<&'static IpcKernel> {
    IPC_KERNEL.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capability::{CapabilityRights, CapabilitySpace, TransferMode},
        object::ObjectRegistry,
    };

    fn install_endpoint(
        space: &CapabilitySpace,
        endpoint: &KernelObjectRef,
        rights: CapabilityRights,
    ) -> CapabilityHandle {
        space
            .install(Arc::clone(endpoint), rights, None)
            .expect("endpoint capability should install")
    }

    #[test]
    fn send_receive_transfers_reduced_capability() {
        let kernel = IpcKernel::new();
        let registry = ObjectRegistry::new();
        let (left, right) = registry.create_channel_pair();
        let shared_memory = registry.create_memory_object(8192, true);
        let sender_space = CapabilitySpace::new();
        let receiver_space = CapabilitySpace::new();

        let left_handle =
            install_endpoint(&sender_space, &left, CapabilityRights::channel_endpoint());
        let right_handle = install_endpoint(
            &receiver_space,
            &right,
            CapabilityRights::channel_endpoint(),
        );
        let memory_handle = sender_space
            .install(shared_memory, CapabilityRights::memory_object(), Some(0x33))
            .expect("memory capability should install");
        let transfer = sender_space
            .prepare_transfer(
                memory_handle,
                CapabilityRights::READ.union(CapabilityRights::MAP),
                TransferMode::Copy,
            )
            .expect("transfer should prepare");
        let message = OutgoingMessage::new(MessageTag(7), &[1, 2, 3])
            .expect("message should fit")
            .add_transfer(transfer)
            .expect("transfer should fit");

        let receipt = kernel
            .send(&sender_space, left_handle, message)
            .expect("send should succeed");
        assert_eq!(receipt.peer, right.id());

        let received = kernel
            .receive(&receiver_space, right_handle)
            .expect("receive should succeed");
        assert_eq!(received.tag, MessageTag(7));
        assert_eq!(received.words, alloc::vec![1, 2, 3]);
        assert_eq!(received.transferred_capabilities.len(), 1);

        let transferred = receiver_space
            .resolve(
                received.transferred_capabilities[0],
                CapabilityRights::READ.union(CapabilityRights::MAP),
            )
            .expect("receiver should get reduced rights");
        assert_eq!(
            transferred.rights,
            CapabilityRights::READ.union(CapabilityRights::MAP)
        );
        assert_eq!(transferred.badge, Some(0x33));

        sender_space
            .resolve(memory_handle, CapabilityRights::WRITE)
            .expect("copy transfer should preserve source capability");
    }

    #[test]
    fn send_rejects_non_channel_reply_endpoint() {
        let kernel = IpcKernel::new();
        let registry = ObjectRegistry::new();
        let (left, _right) = registry.create_channel_pair();
        let event = registry.create_event(false);
        let sender_space = CapabilitySpace::new();

        let left_handle =
            install_endpoint(&sender_space, &left, CapabilityRights::channel_endpoint());
        let event_handle = sender_space
            .install(event, CapabilityRights::event(), None)
            .expect("event capability should install");
        let reply_transfer = sender_space
            .prepare_transfer(event_handle, CapabilityRights::READ, TransferMode::Copy)
            .expect("reply transfer should prepare");
        let message = OutgoingMessage::new(MessageTag(1), &[])
            .expect("message should fit")
            .with_reply_endpoint(reply_transfer);

        assert_eq!(
            kernel.send(&sender_space, left_handle, message),
            Err(IpcError::InvalidReplyEndpoint)
        );
    }

    #[test]
    fn send_rejects_queue_overflow() {
        let kernel = IpcKernel::new();
        let registry = ObjectRegistry::new();
        let (left, right) = registry.create_channel_pair();
        let sender_space = CapabilitySpace::new();
        let receiver_space = CapabilitySpace::new();

        let left_handle =
            install_endpoint(&sender_space, &left, CapabilityRights::channel_endpoint());
        let right_handle = install_endpoint(
            &receiver_space,
            &right,
            CapabilityRights::channel_endpoint(),
        );

        for index in 0..MAX_QUEUED_MESSAGES_PER_ENDPOINT {
            let message =
                OutgoingMessage::new(MessageTag(index as u32), &[index as u64]).expect("fits");
            kernel
                .send(&sender_space, left_handle, message)
                .expect("queue should accept bounded message");
        }

        let overflow = kernel.send(
            &sender_space,
            left_handle,
            OutgoingMessage::new(MessageTag(99), &[99]).expect("fits"),
        );
        assert_eq!(
            overflow,
            Err(IpcError::QueueFull {
                queued_messages: MAX_QUEUED_MESSAGES_PER_ENDPOINT,
                max_messages: MAX_QUEUED_MESSAGES_PER_ENDPOINT,
            })
        );

        let state = right.channel_endpoint().expect("channel object").snapshot();
        assert_eq!(state.queued_messages, MAX_QUEUED_MESSAGES_PER_ENDPOINT);
        let _ = kernel
            .receive(&receiver_space, right_handle)
            .expect("receive should still work");
    }
}
