use spin::Once;

use crate::{
    capability::{CapabilityHandle, CapabilityRights, CapabilitySpace, PreparedTransfer},
    object::{KernelObjectModel, KernelObjectRef, ObjectId},
};

use super::{
    IpcError, MAX_MESSAGE_CAPABILITIES, MAX_QUEUED_MESSAGES_PER_ENDPOINT, MessageReceipt,
    OutgoingMessage, ReceivedMessage, types::MessageEnvelope,
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
        // Snapshot the Move sources before the envelope is consumed so any
        // failure below can restore the sender's handles instead of losing
        // them.
        let mut moved_transfers: [Option<PreparedTransfer>; MAX_MESSAGE_CAPABILITIES + 1] =
            core::array::from_fn(|_| None);
        for (slot, transfer) in moved_transfers
            .iter_mut()
            .zip(message.capabilities.iter().take(message.capability_count))
        {
            *slot = transfer
                .as_ref()
                .filter(|transfer| transfer.moved_source().is_some())
                .cloned();
        }
        moved_transfers[MAX_MESSAGE_CAPABILITIES] = message
            .reply_endpoint
            .as_ref()
            .filter(|transfer| transfer.moved_source().is_some())
            .cloned();

        match self.enqueue(sender_space, endpoint_handle, message) {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                for transfer in moved_transfers.iter().flatten() {
                    sender_space.rollback_moved(transfer);
                }
                Err(error)
            }
        }
    }

    fn enqueue(
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

        // A failure while installing transferred capabilities must not lose
        // the message or the handles installed so far: close the partial
        // installs and put the envelope back at the queue front, preserving
        // FIFO order for other receivers.
        match Self::install_transferred_capabilities(receiver_space, &message) {
            Ok((transferred_capabilities, transferred_capability_count, reply_endpoint)) => {
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
            Err(error) => {
                channel.state.lock().queue.push_front(message).ok();
                Err(error)
            }
        }
    }

    fn install_transferred_capabilities(
        receiver_space: &CapabilitySpace,
        message: &MessageEnvelope,
    ) -> Result<
        (
            [CapabilityHandle; MAX_MESSAGE_CAPABILITIES],
            usize,
            Option<CapabilityHandle>,
        ),
        IpcError,
    > {
        let mut transferred_capabilities = [CapabilityHandle(0); MAX_MESSAGE_CAPABILITIES];
        let mut transferred_capability_count = 0usize;
        let outcome = (|| -> Result<(), IpcError> {
            for transfer in message
                .capabilities
                .iter()
                .take(message.capability_count)
                .flatten()
            {
                let handle = receiver_space.accept_transfer(transfer.clone())?;
                transferred_capabilities[transferred_capability_count] = handle;
                transferred_capability_count += 1;
            }
            Ok(())
        })();

        if let Err(error) = outcome {
            for handle in transferred_capabilities.iter().take(transferred_capability_count) {
                let _ = receiver_space.close(*handle);
            }
            return Err(error);
        }

        let reply_endpoint = match message.reply_endpoint.clone() {
            Some(transfer) => match receiver_space.accept_transfer(transfer) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    for handle in
                        transferred_capabilities.iter().take(transferred_capability_count)
                    {
                        let _ = receiver_space.close(*handle);
                    }
                    return Err(error.into());
                }
            },
            None => None,
        };

        Ok((
            transferred_capabilities,
            transferred_capability_count,
            reply_endpoint,
        ))
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
