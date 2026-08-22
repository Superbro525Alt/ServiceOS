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
        capability::{CapabilityError, CapabilityHandle, CapabilityRights, CapabilitySpace, TransferMode},
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

    #[test]
    fn failed_move_send_restores_sender_handles() {
        let kernel = IpcKernel::new();
        let registry = ObjectRegistry::new();
        let (left, right) = registry.create_channel_pair();
        let payload = registry.create_memory_object(4096, true);
        let sender_space = CapabilitySpace::new();
        let receiver_space = CapabilitySpace::new();

        let left_handle =
            install_endpoint(&sender_space, &left, CapabilityRights::channel_endpoint());
        let _right_handle = install_endpoint(
            &receiver_space,
            &right,
            CapabilityRights::channel_endpoint(),
        );
        let moved_handle = sender_space
            .install(payload, CapabilityRights::memory_object(), Some(9))
            .expect("moved capability should install");

        // Fill the peer queue so the next send fails after preparation.
        for index in 0..MAX_QUEUED_MESSAGES_PER_ENDPOINT {
            let message =
                OutgoingMessage::new(MessageTag(index as u32), &[index as u64]).expect("fits");
            kernel
                .send(&sender_space, left_handle, message)
                .expect("queue should accept bounded message");
        }

        let transfer = sender_space
            .prepare_transfer(moved_handle, CapabilityRights::READ, TransferMode::Move)
            .expect("move prepare should remove the source handle");
        let message = OutgoingMessage::new(MessageTag(5), &[5])
            .expect("fits")
            .add_transfer(transfer)
            .expect("transfer should fit");

        assert!(matches!(
            kernel.send(&sender_space, left_handle, message),
            Err(IpcError::QueueFull { .. })
        ));

        // The failed send must hand the moved handle back to the sender.
        sender_space
            .resolve(moved_handle, CapabilityRights::WRITE)
            .expect("moved handle should be restored after failed send");
    }

    #[test]
    fn failed_receive_keeps_message_and_closes_partial_handles() {
        let kernel = IpcKernel::new();
        let registry = ObjectRegistry::new();
        let (left, right) = registry.create_channel_pair();
        let first_payload = registry.create_memory_object(4096, true);
        let second_payload = registry.create_memory_object(4096, true);
        let sender_space = CapabilitySpace::new();
        let receiver_space = CapabilitySpace::new();

        let left_handle =
            install_endpoint(&sender_space, &left, CapabilityRights::channel_endpoint());
        let right_handle = install_endpoint(
            &receiver_space,
            &right,
            CapabilityRights::channel_endpoint(),
        );

        let first_handle = sender_space
            .install(first_payload, CapabilityRights::memory_object(), None)
            .expect("first payload capability should install");
        let second_handle = sender_space
            .install(second_payload, CapabilityRights::memory_object(), None)
            .expect("second payload capability should install");

        let message = OutgoingMessage::new(MessageTag(11), &[11])
            .expect("fits")
            .add_transfer(
                sender_space
                    .prepare_transfer(first_handle, CapabilityRights::READ, TransferMode::Copy)
                    .expect("first transfer prepares"),
            )
            .expect("first transfer fits")
            .add_transfer(
                sender_space
                    .prepare_transfer(second_handle, CapabilityRights::READ, TransferMode::Copy)
                    .expect("second transfer prepares"),
            )
            .expect("second transfer fits");
        kernel
            .send(&sender_space, left_handle, message)
            .expect("send should succeed");

        // Exhaust the receiver's handle space after exactly one more install
        // so the second capability accept fails mid-message.
        receiver_space.set_next_handle_for_test(u32::MAX - 1);
        assert!(matches!(
            kernel.receive(&receiver_space, right_handle),
            Err(IpcError::Capability(CapabilityError::HandleSpaceExhausted))
        ));
        assert_eq!(
            receiver_space.handle_count(),
            1,
            "partially installed handles must be closed"
        );

        // The popped message must be back at the queue front and intact.
        receiver_space.set_next_handle_for_test(50);
        let received = kernel
            .receive(&receiver_space, right_handle)
            .expect("message should survive the failed receive");
        assert_eq!(received.tag, MessageTag(11));
        assert_eq!(received.transferred_capabilities().len(), 2);
    }
}
