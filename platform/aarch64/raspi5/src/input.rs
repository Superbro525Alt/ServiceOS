//! Raspberry Pi 5 input bring-up.
//!
//! The Pi 5 has no in-tree input transport yet (no USB HID stack, no
//! SPI-attached HID driver), so there is no hardware input backend. The
//! graphical service graph, however, hard-requires a kernel input-source
//! object: session-service validates its startup input handle with
//! `input_source_info` and exits when the handle is not a real input
//! source. To let the graphical graph come up honestly without fabricating
//! input, this module mints a NULL input source (zero capabilities, zero
//! devices, receive always `QueueEmpty`). Session-service classifies a
//! zero-capability source as `ServiceControl` input and runs without
//! hardware input, exactly like the boot-proven peripheral-service
//! client-source path. Operator interaction stays on the serial console
//! until USB HID lands. UNTESTED WITHOUT HARDWARE — nothing here can be
//! exercised until a real Pi 5 boots the image (raspi5 is ManualDeploy in
//! QEMU); the backend is deliberately trivial and matches the trait
//! contract so a future HID driver can replace it behind the same object.

use alloc::sync::Arc;

use serviceos_abi::{InputSourceBackend, InputSourceInfo};
use serviceos_kernel_core::input::{InputBackend, InputSourceError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputStatus {
    pub implemented: bool,
}

pub const fn status() -> InputStatus {
    InputStatus { implemented: false }
}

/// Null input source: reports zero capabilities so consumers degrade to
/// service-controlled input instead of waiting on events that can never
/// arrive. Counts nothing, queues nothing.
pub struct NullInputBackend;

impl InputBackend for NullInputBackend {
    fn info(&self) -> InputSourceInfo {
        InputSourceInfo {
            backend: InputSourceBackend::Unknown as u32,
            capabilities: 0,
            device_count: 0,
            pending_events: 0,
        }
    }

    fn receive(&self) -> Result<serviceos_abi::InputEventInfo, InputSourceError> {
        Err(InputSourceError::QueueEmpty)
    }

    fn poll(&self) -> bool {
        false
    }
}

/// Builds the null input-source backend for the graphical service graph.
pub fn null_backend() -> Arc<dyn InputBackend> {
    Arc::new(NullInputBackend)
}
