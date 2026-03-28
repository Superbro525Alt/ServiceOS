use serviceos_bundle::{
    BOOT_STORE_INDEX_TEXT_MAX, BOOT_STORE_MANIFEST_TEXT_MAX, RestartPolicy, ServiceManifest,
};
use serviceos_userspace_runtime as rt;
use rt::{ServiceId, ServiceImageId};

pub(crate) const MAX_SERVICE_SLOTS: usize = 16;
pub(crate) const MAX_INDEX_BYTES: usize = BOOT_STORE_INDEX_TEXT_MAX;
pub(crate) const MAX_MANIFEST_BYTES: usize = BOOT_STORE_MANIFEST_TEXT_MAX;

#[derive(Clone, Copy)]
pub(crate) struct BootstrapResource {
    pub(crate) handle: rt::Handle,
    pub(crate) len: usize,
    pub(crate) rights: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct BootstrapResources {
    pub(crate) bootstore: BootstrapResource,
    pub(crate) network: Option<BootstrapResource>,
    pub(crate) display: Option<BootstrapResource>,
    pub(crate) input: Option<BootstrapResource>,
}

#[derive(Clone, Copy)]
pub(crate) struct ServiceSlot {
    pub(crate) manifest: ServiceManifest,
    pub(crate) task_handle: rt::Handle,
    pub(crate) control_handle: rt::Handle,
    pub(crate) public_handle: rt::Handle,
    pub(crate) attempts: u32,
    pub(crate) phase: ServicePhase,
    pub(crate) last_exit_code: u64,
    pub(crate) restart_requested: bool,
    pub(crate) occupied: bool,
    pub(crate) dynamic: bool,
}

impl ServiceSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            manifest: ServiceManifest::empty(),
            task_handle: rt::INVALID_HANDLE,
            control_handle: rt::INVALID_HANDLE,
            public_handle: rt::INVALID_HANDLE,
            attempts: 0,
            phase: ServicePhase::Dormant,
            last_exit_code: 0,
            restart_requested: false,
            occupied: false,
            dynamic: false,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ServicePhase {
    Dormant,
    Starting,
    Ready,
    Exited,
}

pub(crate) fn storage_manifest() -> ServiceManifest {
    let mut manifest = ServiceManifest::empty();
    manifest.service_id = ServiceId::Storage;
    manifest.image_id = ServiceImageId::StorageService;
    manifest.restart = RestartPolicy::OnFailure { max_restarts: 1 };
    manifest
}
