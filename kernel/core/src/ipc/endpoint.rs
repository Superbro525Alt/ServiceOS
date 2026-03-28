use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
};
use spin::Mutex;

use crate::object::{KernelObjectRef, KernelObjectWeak};

use super::{ChannelQueueState, types::MessageEnvelope};

pub struct ChannelEndpointObject {
    pub(super) state: Mutex<ChannelEndpointState>,
}

pub(super) struct ChannelEndpointState {
    pub(super) peer: KernelObjectWeak,
    pub(super) queue: VecDeque<MessageEnvelope>,
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
