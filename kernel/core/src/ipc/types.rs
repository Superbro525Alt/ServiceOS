use alloc::vec::Vec;

use crate::{
    capability::{CapabilityError, CapabilityHandle, PreparedTransfer},
    object::ObjectId,
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
    pub(super) tag: MessageTag,
    pub(super) words: Vec<u64>,
    pub(super) capabilities: Vec<PreparedTransfer>,
    pub(super) reply_endpoint: Option<PreparedTransfer>,
    pub(super) shared_memory_hint: Option<SharedMemoryHint>,
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
pub(super) struct MessageEnvelope {
    pub(super) tag: MessageTag,
    pub(super) words: Vec<u64>,
    pub(super) capabilities: Vec<PreparedTransfer>,
    pub(super) reply_endpoint: Option<PreparedTransfer>,
    pub(super) shared_memory_hint: Option<SharedMemoryHint>,
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
