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
    pub(super) word_count: usize,
    pub(super) words: [u64; MAX_MESSAGE_WORDS],
    pub(super) capability_count: usize,
    pub(super) capabilities: [Option<PreparedTransfer>; MAX_MESSAGE_CAPABILITIES],
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

        let mut message = Self {
            tag,
            word_count: words.len(),
            words: [0; MAX_MESSAGE_WORDS],
            capability_count: 0,
            capabilities: core::array::from_fn(|_| None),
            reply_endpoint: None,
            shared_memory_hint: None,
        };
        message.words[..words.len()].copy_from_slice(words);
        Ok(message)
    }

    pub fn add_transfer(mut self, transfer: PreparedTransfer) -> Result<Self, IpcError> {
        if self.capability_count == MAX_MESSAGE_CAPABILITIES {
            return Err(IpcError::TooManyTransfers {
                transfer_count: self.capability_count + 1,
                max_transfers: MAX_MESSAGE_CAPABILITIES,
            });
        }

        self.capabilities[self.capability_count] = Some(transfer);
        self.capability_count += 1;
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
            word_count: self.word_count,
            transfers_capability: self.capability_count != 0 || self.reply_endpoint.is_some(),
        }
    }
}

#[derive(Clone)]
pub(super) struct MessageEnvelope {
    pub(super) tag: MessageTag,
    pub(super) word_count: usize,
    pub(super) words: [u64; MAX_MESSAGE_WORDS],
    pub(super) capability_count: usize,
    pub(super) capabilities: [Option<PreparedTransfer>; MAX_MESSAGE_CAPABILITIES],
    pub(super) reply_endpoint: Option<PreparedTransfer>,
    pub(super) shared_memory_hint: Option<SharedMemoryHint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedMessage {
    pub tag: MessageTag,
    pub word_count: usize,
    pub words: [u64; MAX_MESSAGE_WORDS],
    pub transferred_capability_count: usize,
    pub transferred_capabilities: [CapabilityHandle; MAX_MESSAGE_CAPABILITIES],
    pub reply_endpoint: Option<CapabilityHandle>,
    pub shared_memory_hint: Option<SharedMemoryHint>,
}

impl ReceivedMessage {
    pub fn words(&self) -> &[u64] {
        &self.words[..self.word_count]
    }

    pub fn transferred_capabilities(&self) -> &[CapabilityHandle] {
        &self.transferred_capabilities[..self.transferred_capability_count]
    }
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
