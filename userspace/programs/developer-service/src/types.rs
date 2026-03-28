use serviceos_userspace_runtime as rt;
use rt::{DeveloperArtifactFormat, DeveloperJobState, DeveloperTarget, DeveloperToolchainState};

use crate::consts::{MAX_NAME, MAX_PATH};

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
        if value.len() > N {
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
pub(crate) struct ToolchainSlot {
    pub(crate) occupied: bool,
    pub(crate) target: DeveloperTarget,
    pub(crate) state: DeveloperToolchainState,
    pub(crate) format: DeveloperArtifactFormat,
    pub(crate) name: FixedBytes<MAX_NAME>,
    pub(crate) sdk_root: FixedBytes<MAX_PATH>,
}

impl ToolchainSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            target: DeveloperTarget::NativeX64,
            state: DeveloperToolchainState::Installed,
            format: DeveloperArtifactFormat::ServiceOsFlat,
            name: FixedBytes::empty(),
            sdk_root: FixedBytes::empty(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceSlot {
    pub(crate) occupied: bool,
    pub(crate) name: FixedBytes<MAX_NAME>,
    pub(crate) artifact: FixedBytes<MAX_NAME>,
    pub(crate) source_path: FixedBytes<MAX_PATH>,
    pub(crate) toolchains: [u32; 4],
}

impl WorkspaceSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            name: FixedBytes::empty(),
            artifact: FixedBytes::empty(),
            source_path: FixedBytes::empty(),
            toolchains: [u32::MAX; 4],
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct JobSlot {
    pub(crate) occupied: bool,
    pub(crate) workspace_id: u32,
    pub(crate) target: DeveloperTarget,
    pub(crate) state: DeveloperJobState,
    pub(crate) format: DeveloperArtifactFormat,
    pub(crate) artifact_name: FixedBytes<MAX_NAME>,
    pub(crate) artifact_size: usize,
    pub(crate) artifact_handle: rt::Handle,
    pub(crate) task_handle: rt::Handle,
    pub(crate) report_handle: rt::Handle,
}

impl JobSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            workspace_id: 0,
            target: DeveloperTarget::NativeX64,
            state: DeveloperJobState::Queued,
            format: DeveloperArtifactFormat::ServiceOsFlat,
            artifact_name: FixedBytes::empty(),
            artifact_size: 0,
            artifact_handle: rt::INVALID_HANDLE,
            task_handle: rt::INVALID_HANDLE,
            report_handle: rt::INVALID_HANDLE,
        }
    }
}
