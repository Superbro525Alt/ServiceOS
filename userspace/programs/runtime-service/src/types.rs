use rt::{
    PermissionPolicyState, RuntimeEnvState, RuntimeKind, RuntimeRunState, RuntimeWorkloadKind,
    SecurityAuditKind,
};
use serviceos_userspace_runtime as rt;

use crate::consts::{
    MAX_GUEST_PATH, MAX_LIBS, MAX_MOUNTS, MAX_STORAGE_PATH, MAX_VAR_KEY, MAX_VAR_VALUE, MAX_VARS,
};

#[derive(Clone, Copy, PartialEq)]
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

#[derive(Clone, Copy, PartialEq)]
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

#[derive(Clone, Copy, PartialEq)]
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

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct LibSlot {
    pub(crate) name: FixedBytes<MAX_VAR_KEY>,
    pub(crate) guest: FixedBytes<MAX_GUEST_PATH>,
}

impl LibSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            name: FixedBytes::empty(),
            guest: FixedBytes::empty(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Profile {
    pub(crate) kind: RuntimeKind,
    pub(crate) capabilities: u32,
    /// Sensitive capability requests declared through the env profile's
    /// `requests` line (`requests = network,graphics,audio`). These are
    /// sensitive-only words that join `capabilities` when the environment
    /// is instantiated so the approval matrix sees them as requested.
    pub(crate) requested_caps: u32,
    /// Env-profile line `linux-syscall = true`: guest executables spawned
    /// by this environment's runs enter the kernel through Linux x86_64
    /// syscall-number translation instead of native ServiceOS numbering.
    pub(crate) linux_syscall: bool,
    pub(crate) mounts: [MountSlot; MAX_MOUNTS],
    pub(crate) mount_count: usize,
    pub(crate) vars: [VarSlot; MAX_VARS],
    pub(crate) var_count: usize,
    pub(crate) libs: [LibSlot; MAX_LIBS],
    pub(crate) lib_count: usize,
}

impl Profile {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: RuntimeKind::Posix,
            capabilities: 0,
            requested_caps: 0,
            linux_syscall: false,
            mounts: [MountSlot::empty(); MAX_MOUNTS],
            mount_count: 0,
            vars: [VarSlot::empty(); MAX_VARS],
            var_count: 0,
            libs: [LibSlot::empty(); MAX_LIBS],
            lib_count: 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct EnvSlot {
    pub(crate) occupied: bool,
    pub(crate) kind: RuntimeKind,
    pub(crate) state: RuntimeEnvState,
    pub(crate) capabilities: u32,
    pub(crate) granted_caps: u32,
    /// Guest syscall ABI mode inherited from the profile's `linux-syscall`
    /// line; surfaced through the additive EnvStatusReply word.
    pub(crate) linux_syscall: bool,
    pub(crate) sandbox: crate::sandbox::SandboxProfile,
    pub(crate) mounts: [MountSlot; MAX_MOUNTS],
    pub(crate) mount_count: usize,
    pub(crate) vars: [VarSlot; MAX_VARS],
    pub(crate) var_count: usize,
    pub(crate) libs: [LibSlot; MAX_LIBS],
    pub(crate) lib_count: usize,
    pub(crate) active_runs: u32,
    /// Additive S11 trailing field: the workload sandbox manifest associated
    /// with this environment (latched on the first manifest-carrying launch,
    /// matched exactly afterwards). Absent means the launch gate behaves
    /// exactly as before the manifest existed.
    pub(crate) manifest: Option<crate::sandbox::SandboxManifest>,
    /// Additive cross-reboot persistence fields: boot-local monotonic ticks
    /// captured at record creation and last durable mutation. Ticks reset
    /// every boot (honest stamping, not wall-clock time); the cross-reboot
    /// store carries them so operators can order record ages within a boot.
    pub(crate) created_tick: u64,
    pub(crate) updated_tick: u64,
}

impl EnvSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            kind: RuntimeKind::Posix,
            state: RuntimeEnvState::Destroyed,
            capabilities: 0,
            granted_caps: 0,
            linux_syscall: false,
            sandbox: crate::sandbox::SandboxProfile::empty(),
            mounts: [MountSlot::empty(); MAX_MOUNTS],
            mount_count: 0,
            vars: [VarSlot::empty(); MAX_VARS],
            var_count: 0,
            libs: [LibSlot::empty(); MAX_LIBS],
            lib_count: 0,
            active_runs: 0,
            manifest: None,
            created_tick: 0,
            updated_tick: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RunSlot {
    pub(crate) occupied: bool,
    pub(crate) env_id: u32,
    pub(crate) workload: RuntimeWorkloadKind,
    /// True when the run launched a guest image through the raw-image
    /// spawn path instead of the hosted posix tool.
    pub(crate) guest_exec: bool,
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
            guest_exec: false,
            state: RuntimeRunState::Exited,
            task_handle: rt::INVALID_HANDLE,
            session_handle: rt::INVALID_HANDLE,
            exit_code: 0,
        }
    }

    /// Workload word reported over IPC. Guest-exec runs surface the
    /// runtime-service-local exec marker (see `abi_image`).
    pub(crate) fn workload_word(&self) -> u64 {
        if self.guest_exec {
            crate::abi_image::EXEC_GUEST_WORKLOAD as u64
        } else {
            self.workload as u32 as u64
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AuditSlot {
    pub(crate) occupied: bool,
    pub(crate) sequence: u32,
    pub(crate) kind: SecurityAuditKind,
    pub(crate) env_id: u32,
    pub(crate) capabilities: u32,
    pub(crate) detail: u64,
    pub(crate) policy: PermissionPolicyState,
}

impl AuditSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            sequence: 0,
            kind: SecurityAuditKind::RuntimeApprovalRequested,
            env_id: 0,
            capabilities: 0,
            detail: 0,
            policy: PermissionPolicyState::DefaultAllow,
        }
    }
}
