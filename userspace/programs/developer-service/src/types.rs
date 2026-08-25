use rt::{DeveloperArtifactFormat, DeveloperJobState, DeveloperTarget, DeveloperToolchainState};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_NAME, MAX_PATH},
    payload::{PayloadSlot, MAX_PAYLOADS},
    routing::{self, BuildRoute, ExecutionMode},
    sandbox::SandboxDecision,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

/// Where a job's build output lives: local artifact handle once the worker
/// reports one, or an exported-pending reference at the remote farm
/// endpoint that owns the artifact.
#[derive(Clone, Copy)]
pub(crate) enum ExportState {
    Local,
    PendingRemote { endpoint: FixedBytes<MAX_PATH> },
}

#[derive(Clone, Copy)]
pub(crate) struct ToolchainSlot {
    pub(crate) occupied: bool,
    pub(crate) target: DeveloperTarget,
    pub(crate) state: DeveloperToolchainState,
    pub(crate) format: DeveloperArtifactFormat,
    pub(crate) name: FixedBytes<MAX_NAME>,
    pub(crate) sdk_root: FixedBytes<MAX_PATH>,
    /// Optional connection config from the descriptor (`remote_endpoint=`),
    /// e.g. "farm@10.0.0.9:7900"; empty until a descriptor provides it.
    pub(crate) remote_endpoint: FixedBytes<MAX_PATH>,
    /// Payload blobs declared by the descriptor (`payload=name@ref` lines);
    /// materialized into the writable SDK mirror at install time.
    pub(crate) payloads: [PayloadSlot; MAX_PAYLOADS],
    pub(crate) payload_count: usize,
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
            remote_endpoint: FixedBytes::empty(),
            payloads: [PayloadSlot::empty(); MAX_PAYLOADS],
            payload_count: 0,
        }
    }

    pub(crate) fn configured(&self) -> bool {
        !self.remote_endpoint.as_bytes().is_empty()
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
    pub(crate) sandbox: SandboxDecision,
    pub(crate) route: BuildRoute,
    /// How the worker actually ran for this job (direct spawn vs routed
    /// environment exec vs routed-then-fallback), recorded at launch.
    pub(crate) mode: ExecutionMode,
    pub(crate) export: ExportState,
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
            sandbox: SandboxDecision {
                allowed: false,
                scope_count: 0,
            },
            route: routing::BuildRoute::DirectSpawn,
            mode: routing::ExecutionMode::DirectSpawn,
            export: ExportState::Local,
        }
    }

    pub(crate) fn exported_pending(&self) -> bool {
        matches!(self.export, ExportState::PendingRemote { .. })
    }

    pub(crate) fn endpoint_bytes(&self) -> &[u8] {
        match &self.export {
            ExportState::Local => &[],
            ExportState::PendingRemote { endpoint } => endpoint.as_bytes(),
        }
    }
}
