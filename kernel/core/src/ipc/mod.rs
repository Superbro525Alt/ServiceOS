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
pub enum IpcError {
    UnsupportedInPhase0,
    EndpointNotReady,
    BufferShapeInvalid,
}

pub trait IpcTransport {
    fn send(
        &self,
        endpoint: EndpointId,
        tag: MessageTag,
        buffer: MessageBufferDescriptor,
    ) -> Result<(), IpcError>;
}
