use alloc::{sync::Arc, vec::Vec};
use spin::{Mutex, Once};

use serviceos_abi::{
    InputButton, InputDeviceInfo, InputEventInfo, InputEventKind, InputSourceBackend,
    InputSourceInfo, input_capability, input_device_class, input_role_flag,
};
use serviceos_kernel_core::{
    input::{InputBackend, InputSourceError},
    object::ObjectId,
    task,
};
use virtio_drivers::{
    device::input::{InputEvent, VirtIOInput},
    transport::DeviceType,
};

use crate::dtb::VirtioMmioDevice;
use crate::virtio::{KernelHal, VirtioTransport, discover};

const MAX_PENDING_EVENTS: usize = 128;
/// Upper bound on distinctly enumerated physical input instances reported at
/// bring-up.
const MAX_INPUT_INSTANCES: usize = 8;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0x00;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputBringupSummary {
    pub backend: InputSourceBackend,
    pub keyboard_devices: u32,
    pub pointer_devices: u32,
    /// Per-instance enumeration (id + semantic role), not just counts.
    pub instances: [InputInstanceSummary; MAX_INPUT_INSTANCES],
    pub instance_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputInstanceSummary {
    pub source_id: u32,
    /// One of `serviceos_abi::input_device_class`.
    pub class: u32,
    /// Bitmask of `serviceos_abi::input_role_flag`.
    pub role_flags: u32,
}

pub fn initialize(devices: &[VirtioMmioDevice]) -> Option<Arc<dyn InputBackend>> {
    let transports = discover(devices, DeviceType::Input);
    if transports.is_empty() {
        return None;
    }

    let mut input_devices = Vec::new();
    let mut instances = [InputInstanceSummary {
        source_id: 0,
        class: 0,
        role_flags: 0,
    }; MAX_INPUT_INSTANCES];
    let mut instance_count = 0usize;
    let mut keyboard_devices = 0u32;
    let mut pointer_devices = 0u32;

    for (_, transport) in transports {
        let Ok(mut device) = VirtIOInput::<KernelHal, VirtioTransport>::new(transport) else {
            continue;
        };
        let pointer = pointer_axes(&mut device).ok().flatten();
        let has_keys = device
            .ev_bits(EV_KEY as u8)
            .ok()
            .is_some_and(|bits| !bits.is_empty());

        if pointer.is_some() {
            pointer_devices = pointer_devices.saturating_add(1);
        }
        if has_keys {
            keyboard_devices = keyboard_devices.saturating_add(1);
        }

        let source_id = (input_devices.len() + 1) as u32;
        let (class, role_flags) = instance_signature(pointer.as_ref(), has_keys);
        if instance_count < MAX_INPUT_INSTANCES {
            instances[instance_count] = InputInstanceSummary {
                source_id,
                class,
                role_flags,
            };
            instance_count += 1;
        }
        input_devices.push(InputDeviceState {
            device,
            source_id,
            present: true,
            pointer,
            keyboard: has_keys,
            pending_x: 0,
            pending_y: 0,
            motion_dirty: false,
        });
    }

    if input_devices.is_empty() {
        return None;
    }

    let _ = BRINGUP_SUMMARY.call_once(|| InputBringupSummary {
        backend: InputSourceBackend::VirtioPci,
        keyboard_devices,
        pointer_devices,
        instances,
        instance_count,
    });

    Some(Arc::new(VirtioInputBackend::new(
        input_devices,
        keyboard_devices,
        pointer_devices,
    )))
}

pub fn bringup_summary() -> Option<InputBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

pub fn poll_ready_sources() {
    if let Some(manager) = serviceos_kernel_core::input::manager() {
        manager.poll_ready(|object_id| {
            let _ = task::notify_input_ready(ObjectId(object_id));
        });
    }
}

static BRINGUP_SUMMARY: Once<InputBringupSummary> = Once::new();

struct VirtioInputBackend {
    state: Mutex<VirtioInputState>,
}

struct VirtioInputState {
    devices: Vec<InputDeviceState>,
    queue: [InputEventInfo; MAX_PENDING_EVENTS],
    queue_head: usize,
    queue_len: usize,
    capabilities: u32,
}

struct InputDeviceState {
    device: VirtIOInput<KernelHal, VirtioTransport>,
    /// Stable instance id (1-based bring-up order); matches `source_id` on
    /// every event this device emits.
    source_id: u32,
    /// Hot-plug presence: absent instances are skipped entirely by polling
    /// so a removed device cannot wedge the event pipeline.
    present: bool,
    pointer: Option<PointerSource>,
    keyboard: bool,
    pending_x: i32,
    pending_y: i32,
    motion_dirty: bool,
}

#[derive(Clone, Copy)]
enum PointerSource {
    Absolute(PointerAxes),
    Relative,
}

#[derive(Clone, Copy)]
struct PointerAxes {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
}

impl VirtioInputBackend {
    fn new(devices: Vec<InputDeviceState>, keyboard_devices: u32, pointer_devices: u32) -> Self {
        let mut capabilities = 0u32;
        if pointer_devices > 0 {
            capabilities |= input_capability::POINTER;
        }
        if keyboard_devices > 0 {
            capabilities |= input_capability::KEYBOARD;
        }

        Self {
            state: Mutex::new(VirtioInputState {
                devices,
                queue: [InputEventInfo {
                    kind: 0,
                    code: 0,
                    value0: 0,
                    value1: 0,
                    source_id: 0,
                }; MAX_PENDING_EVENTS],
                queue_head: 0,
                queue_len: 0,
                capabilities,
            }),
        }
    }
}

impl InputBackend for VirtioInputBackend {
    fn info(&self) -> InputSourceInfo {
        let state = self.state.lock();
        InputSourceInfo {
            backend: InputSourceBackend::VirtioPci as u32,
            capabilities: state.capabilities,
            device_count: state
                .devices
                .iter()
                .filter(|device| device.present)
                .count() as u32,
            pending_events: state.queue_len as u32,
        }
    }

    fn enumerate_devices(&self) -> Vec<InputDeviceInfo> {
        self.state
            .lock()
            .devices
            .iter()
            .map(|device| {
                let (class, role_flags) =
                    instance_signature(device.pointer.as_ref(), device.keyboard);
                InputDeviceInfo {
                    source_id: device.source_id,
                    class,
                    role_flags,
                    present: u32::from(device.present),
                }
            })
            .collect()
    }

    fn set_device_present(&self, source_id: u32, present: bool) {
        let mut state = self.state.lock();
        for device in state.devices.iter_mut() {
            if device.source_id == source_id {
                device.present = present;
            }
        }
    }

    fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
        let mut state = self.state.lock();
        if state.queue_len == 0 {
            return Err(InputSourceError::QueueEmpty);
        }
        let event = state.queue[state.queue_head];
        state.queue_head = (state.queue_head + 1) % MAX_PENDING_EVENTS;
        state.queue_len -= 1;
        Ok(event)
    }

    fn poll(&self) -> bool {
        let mut state = self.state.lock();
        let mut became_ready = false;

        for device_index in 0..state.devices.len() {
            // Hot-plug guard: absent instances are never acked or drained.
            if !state.devices[device_index].present {
                continue;
            }
            {
                let device = &mut state.devices[device_index];
                let _ = device.device.ack_interrupt();
            }
            loop {
                let normalized = {
                    let device = &mut state.devices[device_index];
                    device
                        .device
                        .pop_pending_event()
                        .and_then(|event| normalize_event(device, event))
                        .map(|mut info| {
                            info.source_id = state.devices[device_index].source_id;
                            info
                        })
                };
                let Some(normalized) = normalized else {
                    break;
                };
                if state.queue_len == MAX_PENDING_EVENTS {
                    state.queue_head = (state.queue_head + 1) % MAX_PENDING_EVENTS;
                    state.queue_len -= 1;
                }
                if state.queue_len == 0 {
                    became_ready = true;
                }
                let insert_index = (state.queue_head + state.queue_len) % MAX_PENDING_EVENTS;
                state.queue[insert_index] = normalized;
                state.queue_len += 1;
            }
        }

        became_ready
    }
}

/// Semantic class/role-flag pair reported by per-instance enumeration.
fn instance_signature(
    pointer: Option<&PointerSource>,
    has_keys: bool,
) -> (u32, u32) {
    match pointer {
        Some(PointerSource::Absolute(_)) => (
            input_device_class::TABLET,
            input_role_flag::POSITIONAL_AUTHORITY,
        ),
        Some(PointerSource::Relative) => (
            input_device_class::POINTER,
            input_role_flag::POSITIONAL_AUTHORITY,
        ),
        None => (input_device_class::KEYBOARD, 0),
    }
}

fn pointer_axes(
    device: &mut VirtIOInput<KernelHal, VirtioTransport>,
) -> Result<Option<PointerSource>, ()> {
    if let (Ok(x), Ok(y)) = (device.abs_info(ABS_X as u8), device.abs_info(ABS_Y as u8)) {
        if x.max > x.min && y.max > y.min {
            return Ok(Some(PointerSource::Absolute(PointerAxes {
                min_x: x.min as i32,
                max_x: x.max as i32,
                min_y: y.min as i32,
                max_y: y.max as i32,
            })));
        }
    }

    let rel_bits = device.ev_bits(EV_REL as u8).map_err(|_| ())?;
    let has_rel_x = bit_is_set(&rel_bits, REL_X as usize);
    let has_rel_y = bit_is_set(&rel_bits, REL_Y as usize);
    if has_rel_x || has_rel_y {
        return Ok(Some(PointerSource::Relative));
    }

    Ok(None)
}

fn normalize_event(device: &mut InputDeviceState, event: InputEvent) -> Option<InputEventInfo> {
    match event.event_type {
        EV_ABS => {
            if let Some(PointerSource::Absolute(_)) = device.pointer {
                match event.code {
                    ABS_X => {
                        device.pending_x = event.value as i32;
                        device.motion_dirty = true;
                    }
                    ABS_Y => {
                        device.pending_y = event.value as i32;
                        device.motion_dirty = true;
                    }
                    _ => {}
                }
            }
            None
        }
        EV_REL => {
            if device.pointer.is_some() {
                match event.code {
                    REL_X if matches!(device.pointer, Some(PointerSource::Relative)) => {
                        device.pending_x = device.pending_x.saturating_add(event.value as i32);
                        device.motion_dirty = true;
                    }
                    REL_Y if matches!(device.pointer, Some(PointerSource::Relative)) => {
                        device.pending_y = device.pending_y.saturating_add(event.value as i32);
                        device.motion_dirty = true;
                    }
                    REL_WHEEL => {
                        return Some(InputEventInfo {
                            source_id: 2,
                            kind: InputEventKind::PointerScroll as u32,
                            code: 0,
                            value0: 0,
                            value1: event.value as i32,
                        });
                    }
                    _ => {}
                }
            }
            None
        }
        EV_SYN if event.code == SYN_REPORT => {
            let pointer = device.pointer?;
            if !device.motion_dirty {
                return None;
            }
            device.motion_dirty = false;
            match pointer {
                PointerSource::Absolute(axes) => Some(InputEventInfo {
                    source_id: 3,
                    kind: InputEventKind::PointerMotion as u32,
                    code: 0,
                    value0: normalize_axis(device.pending_x, axes.min_x, axes.max_x),
                    value1: normalize_axis(device.pending_y, axes.min_y, axes.max_y),
                }),
                PointerSource::Relative => {
                    let delta_x = device.pending_x;
                    let delta_y = device.pending_y;
                    device.pending_x = 0;
                    device.pending_y = 0;
                    Some(InputEventInfo {
                        source_id: 2,
                        kind: InputEventKind::PointerDelta as u32,
                        code: 0,
                        value0: delta_x,
                        value1: delta_y,
                    })
                }
            }
        }
        EV_KEY if device.pointer.is_some() => map_pointer_button(event),
        EV_KEY if device.keyboard => Some(InputEventInfo {
            source_id: 1,
            kind: InputEventKind::Key as u32,
            code: event.code as u32,
            value0: if event.value == 0 { 0 } else { 1 },
            value1: 0,
        }),
        _ => None,
    }
}

fn map_pointer_button(event: InputEvent) -> Option<InputEventInfo> {
    let button = match event.code {
        BTN_LEFT => InputButton::Left,
        BTN_RIGHT => InputButton::Right,
        BTN_MIDDLE => InputButton::Middle,
        _ => return None,
    };
    Some(InputEventInfo {
        source_id: 2,
        kind: InputEventKind::PointerButton as u32,
        code: button as u32,
        value0: if event.value == 0 { 0 } else { 1 },
        value1: 0,
    })
}

fn normalize_axis(value: i32, min: i32, max: i32) -> i32 {
    if max <= min {
        return 0;
    }
    let span = (max - min) as i64;
    let clamped = value.clamp(min, max) as i64 - min as i64;
    ((clamped.saturating_mul(65_535)) / span) as i32
}

fn bit_is_set(bits: &[u8], index: usize) -> bool {
    let byte = index / 8;
    let bit = index % 8;
    bits.get(byte).is_some_and(|value| value & (1 << bit) != 0)
}
