use spin::Once;

use crate::{
    capability::{CapabilityHandle, CapabilityRights, CapabilitySpace},
    object::{KernelObjectModel, KernelObjectRef, ObjectId},
};

use super::{
    IpcError, MAX_QUEUED_MESSAGES_PER_ENDPOINT, MessageReceipt, OutgoingMessage, ReceivedMessage,
    types::MessageEnvelope,
};

pub struct IpcKernel;

impl IpcKernel {
    pub(crate) fn new() -> Self {
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

        peer_state
            .queue
            .push_back(MessageEnvelope {
                tag: message.tag,
                word_count: message.word_count,
                words: message.words,
                capability_count: message.capability_count,
                capabilities: message.capabilities,
                reply_endpoint: message.reply_endpoint,
                shared_memory_hint: message.shared_memory_hint,
            })
            .map_err(|_| IpcError::QueueFull {
                queued_messages: MAX_QUEUED_MESSAGES_PER_ENDPOINT,
                max_messages: MAX_QUEUED_MESSAGES_PER_ENDPOINT,
            })?;
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

        let mut transferred_capabilities = [CapabilityHandle(0); super::MAX_MESSAGE_CAPABILITIES];
        let mut transferred_capability_count = 0usize;
        for transfer in message
            .capabilities
            .into_iter()
            .take(message.capability_count)
        {
            let handle = receiver_space.accept_transfer(
                transfer.expect("capability slots up to capability_count are populated"),
            )?;
            transferred_capabilities[transferred_capability_count] = handle;
            transferred_capability_count += 1;
        }
        let reply_endpoint = message
            .reply_endpoint
            .map(|transfer| receiver_space.accept_transfer(transfer))
            .transpose()?;

        Ok(ReceivedMessage {
            tag: message.tag,
            word_count: message.word_count,
            words: message.words,
            transferred_capability_count,
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
