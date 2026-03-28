mod endpoint;
mod kernel;
mod types;

pub use endpoint::ChannelEndpointObject;
pub use kernel::{IpcKernel, initialize, kernel};
pub use types::{
    ChannelQueueState, EndpointId, IpcError, IpcTransport, MAX_MESSAGE_CAPABILITIES,
    MAX_MESSAGE_WORDS, MAX_QUEUED_MESSAGES_PER_ENDPOINT, MessageBufferDescriptor, MessageReceipt,
    MessageTag, OutgoingMessage, ReceivedMessage, SharedMemoryHint,
};

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::{
        capability::{CapabilityHandle, CapabilityRights, CapabilitySpace, TransferMode},
        object::{KernelObjectRef, ObjectRegistry},
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
        assert_eq!(received.words(), &[1, 2, 3]);
        assert_eq!(received.transferred_capabilities().len(), 1);

        let transferred = receiver_space
            .resolve(
                received.transferred_capabilities()[0],
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
