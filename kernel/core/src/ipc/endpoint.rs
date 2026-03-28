use alloc::sync::{Arc, Weak};
use spin::Mutex;

use crate::object::{KernelObjectRef, KernelObjectWeak};

use super::{ChannelQueueState, MAX_QUEUED_MESSAGES_PER_ENDPOINT, types::MessageEnvelope};

pub struct ChannelEndpointObject {
    pub(super) state: Mutex<ChannelEndpointState>,
}

pub(super) struct ChannelEndpointState {
    pub(super) peer: KernelObjectWeak,
    pub(super) queue: MessageQueue,
}

pub(super) struct MessageQueue {
    slots: [Option<MessageEnvelope>; MAX_QUEUED_MESSAGES_PER_ENDPOINT],
    head: usize,
    len: usize,
}

impl MessageQueue {
    fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push_back(&mut self, envelope: MessageEnvelope) -> Result<(), MessageEnvelope> {
        if self.len == self.slots.len() {
            return Err(envelope);
        }

        let index = (self.head + self.len) % self.slots.len();
        self.slots[index] = Some(envelope);
        self.len += 1;
        Ok(())
    }

    pub fn pop_front(&mut self) -> Option<MessageEnvelope> {
        if self.len == 0 {
            return None;
        }

        let envelope = self.slots[self.head].take();
        self.head = (self.head + 1) % self.slots.len();
        self.len -= 1;
        envelope
    }
}

impl ChannelEndpointObject {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ChannelEndpointState {
                peer: Weak::new(),
                queue: MessageQueue::new(),
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
