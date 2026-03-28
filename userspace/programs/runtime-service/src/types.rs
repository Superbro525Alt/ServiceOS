use serviceos_userspace_runtime as rt;
use rt::{RuntimeEnvState, RuntimeKind, RuntimeRunState, RuntimeWorkloadKind};

use crate::consts::{
    MAX_GUEST_PATH, MAX_MOUNTS, MAX_STORAGE_PATH, MAX_VARS, MAX_VAR_KEY, MAX_VAR_VALUE,
};

#[derive(Clone, Copy)]
pub(crate) struct FixedBytes<const N: usize> {
    pub(crate) len: usize,
    pub(crate) bytes: [u8; N],
}

impl<const N: usize> FixedBytes<N> {
    pub(crate) const fn empty() -> Self {
        Self {
            len: 0,
            bytes: [0; N],
        }
    }

    pub(crate) fn set(&mut self, value: &[u8]) -> rt::Result<()> {
        if value.len() > self.bytes.len() {
            return Err(rt::Error::BufferTooSmall);
        }
        self.bytes[..value.len()].copy_from_slice(value);
        self.len = value.len();
        Ok(())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MountSlot {
    pub(crate) guest: FixedBytes<MAX_GUEST_PATH>,
    pub(crate) source: FixedBytes<MAX_STORAGE_PATH>,
}

impl MountSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            guest: FixedBytes::empty(),
            source: FixedBytes::empty(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VarSlot {
    pub(crate) key: FixedBytes<MAX_VAR_KEY>,
    pub(crate) value: FixedBytes<MAX_VAR_VALUE>,
}

impl VarSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            key: FixedBytes::empty(),
            value: FixedBytes::empty(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Profile {
    pub(crate) kind: RuntimeKind,
    pub(crate) capabilities: u32,
    pub(crate) mounts: [MountSlot; MAX_MOUNTS],
    pub(crate) mount_count: usize,
    pub(crate) vars: [VarSlot; MAX_VARS],
    pub(crate) var_count: usize,
}

impl Profile {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: RuntimeKind::Posix,
            capabilities: 0,
            mounts: [MountSlot::empty(); MAX_MOUNTS],
            mount_count: 0,
            vars: [VarSlot::empty(); MAX_VARS],
            var_count: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EnvSlot {
    pub(crate) occupied: bool,
    pub(crate) kind: RuntimeKind,
    pub(crate) state: RuntimeEnvState,
    pub(crate) capabilities: u32,
    pub(crate) mounts: [MountSlot; MAX_MOUNTS],
    pub(crate) mount_count: usize,
    pub(crate) vars: [VarSlot; MAX_VARS],
    pub(crate) var_count: usize,
    pub(crate) active_runs: u32,
}

impl EnvSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            kind: RuntimeKind::Posix,
            state: RuntimeEnvState::Destroyed,
            capabilities: 0,
            mounts: [MountSlot::empty(); MAX_MOUNTS],
            mount_count: 0,
            vars: [VarSlot::empty(); MAX_VARS],
            var_count: 0,
            active_runs: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RunSlot {
    pub(crate) occupied: bool,
    pub(crate) env_id: u32,
    pub(crate) workload: RuntimeWorkloadKind,
    pub(crate) state: RuntimeRunState,
    pub(crate) task_handle: rt::Handle,
    pub(crate) session_handle: rt::Handle,
    pub(crate) exit_code: u64,
}

impl RunSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            env_id: 0,
            workload: RuntimeWorkloadKind::Inspect,
            state: RuntimeRunState::Exited,
            task_handle: rt::INVALID_HANDLE,
            session_handle: rt::INVALID_HANDLE,
            exit_code: 0,
        }
    }
}
