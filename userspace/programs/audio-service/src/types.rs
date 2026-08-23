use rt::AudioStreamState;
use serviceos_userspace_runtime as rt;

#[derive(Clone, Copy)]
pub(crate) struct StreamSlot {
    pub(crate) active: bool,
    pub(crate) control_handle: rt::Handle,
    pub(crate) session_id: u32,
    pub(crate) endpoint_index: u32,
    pub(crate) frequency_hz: u32,
    pub(crate) until_tick: u64,
    pub(crate) state: AudioStreamState,
}

impl StreamSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            active: false,
            control_handle: rt::INVALID_HANDLE,
            session_id: 0,
            endpoint_index: 0,
            frequency_hz: 0,
            until_tick: 0,
            state: AudioStreamState::Closed,
        }
    }
}
