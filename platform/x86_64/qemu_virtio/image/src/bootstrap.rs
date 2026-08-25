use serviceos_abi::{ControlTag, ServiceImageId, bootstrap_resource};
use serviceos_bundle::BootStore;
use serviceos_kernel_arch_x86_64::user;
use serviceos_kernel_core::{
    Kernel,
    capability::{CapabilityError, CapabilityRights, TransferMode},
    ipc::{self, IpcError, MessageTag, OutgoingMessage},
    task::{SchedulerError, TaskRole, ThreadId},
    user::{self as kernel_user, SpawnError, TaskExitStatus},
};

use crate::{BOOT_STORE_IMAGE_SOURCE, executor::run_userspace_executor, logging::log_line};
/// Boot-mode word passed to the root-manager in the startup message
/// (3 = recovery; see root-manager bootmode). Selected at build time via
/// SERVICEOS_BOOT_MODE=recovery, e.g. `cargo xtask recover`.
fn root_boot_mode_word() -> u64 {
    if option_env!("SERVICEOS_BOOT_MODE") == Some("recovery") {
        3
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BootstrapError {
    RootSpawn(SpawnError),
    Scheduler(SchedulerError),
    Capability(CapabilityError),
    Ipc(IpcError),
    MissingRootTask,
    MissingRootThread,
    MissingBootStore,
    UserRun(user::UserLaunchError),
}

impl From<SpawnError> for BootstrapError {
    fn from(error: SpawnError) -> Self {
        Self::RootSpawn(error)
    }
}

impl From<SchedulerError> for BootstrapError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl From<CapabilityError> for BootstrapError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<IpcError> for BootstrapError {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

impl From<user::UserLaunchError> for BootstrapError {
    fn from(error: user::UserLaunchError) -> Self {
        Self::UserRun(error)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RootBootstrapSummary {
    pub(crate) root_task: u64,
    pub(crate) root_thread: ThreadId,
    pub(crate) exit_status: TaskExitStatus,
    pub(crate) scheduler_current: Option<ThreadId>,
    pub(crate) runnable_threads: usize,
    pub(crate) blocked_threads: usize,
    pub(crate) context_switches: u64,
}

pub(crate) fn launch_root_manager(
    kernel: &Kernel<'_>,
    bootstrap_block: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_network: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_display: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_input: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_audio: Option<serviceos_kernel_core::object::KernelObjectRef>,
) -> Result<RootBootstrapSummary, BootstrapError> {
    log_line("bootstrap", "preparing root-manager bootstrap channel");
    let ipc_kernel = ipc::kernel().ok_or(BootstrapError::MissingBootStore)?;
    let bootstrap_task = kernel
        .objects()
        .bootstrap_task()
        .task()
        .ok_or(BootstrapError::MissingRootTask)?;
    let (kernel_bootstrap_endpoint, root_bootstrap_endpoint) =
        ipc_kernel.create_channel_pair(kernel.objects());
    let kernel_bootstrap_handle = bootstrap_task.capability_space().install(
        kernel_bootstrap_endpoint,
        CapabilityRights::channel_endpoint(),
        None,
    )?;
    let root_bootstrap_handle = bootstrap_task.capability_space().install(
        root_bootstrap_endpoint,
        CapabilityRights::channel_endpoint(),
        None,
    )?;
    let root_bootstrap_transfer = bootstrap_task.capability_space().prepare_transfer(
        root_bootstrap_handle,
        CapabilityRights::channel_endpoint(),
        TransferMode::Move,
    )?;
    log_line(
        "bootstrap",
        "creating root-manager boot-store and authority transfers",
    );
    let boot_store_bytes = kernel
        .boot_context()
        .boot_store
        .ok_or(BootstrapError::MissingBootStore)?;
    let boot_store_object = kernel
        .objects()
        .registry()
        .create_memory_object_from_bytes(boot_store_bytes);
    let boot_store_handle = bootstrap_task.capability_space().install(
        boot_store_object,
        CapabilityRights::READ
            .union(CapabilityRights::DUPLICATE)
            .union(CapabilityRights::TRANSFER),
        None,
    )?;
    let bootstrap_authority_handle = bootstrap_task.capability_space().install(
        kernel.objects().bootstrap_capability().clone(),
        CapabilityRights::bootstrap().union(CapabilityRights::TRANSFER),
        None,
    )?;
    let boot_store_transfer = bootstrap_task.capability_space().prepare_transfer(
        boot_store_handle,
        CapabilityRights::READ
            .union(CapabilityRights::DUPLICATE)
            .union(CapabilityRights::TRANSFER),
        TransferMode::Copy,
    )?;
    let bootstrap_authority_transfer = bootstrap_task.capability_space().prepare_transfer(
        bootstrap_authority_handle,
        CapabilityRights::bootstrap(),
        TransferMode::Move,
    )?;
    let network_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_network,
        CapabilityRights::packet_interface(),
    )?;
    let block_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_block,
        CapabilityRights::block_device(),
    )?;
    let display_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_display,
        CapabilityRights::display_output(),
    )?;
    let input_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_input,
        CapabilityRights::input_source(),
    )?;
    let audio_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_audio,
        CapabilityRights::audio_endpoint(),
    )?;

    log_line("bootstrap", "spawning root-manager task");
    let root = kernel_user::spawn_builtin_task(
        ServiceImageId::RootManager as u32,
        TaskRole::SystemService,
        Some(root_bootstrap_transfer),
    )?;
    log_line("bootstrap", "sending root-manager startup message");
    let mut bootstrap_resource_flags = 0u64;
    if block_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::BLOCK;
    }
    if network_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::NETWORK;
    }
    if display_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::DISPLAY;
    }
    if input_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::INPUT;
    }
    if audio_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::AUDIO;
    }
    let mut startup = OutgoingMessage::new(
        MessageTag(ControlTag::Startup as u32),
        &[
            boot_store_bytes.len() as u64,
            serviceos_abi::BootstrapPlatform::QemuVirtio as u32 as u64,
            bootstrap_resource_flags,
            root_boot_mode_word(),
        ],
    )?
    .add_transfer(boot_store_transfer)?
    .add_transfer(bootstrap_authority_transfer)?;
    if let Some(block_transfer) = block_transfer {
        startup = startup.add_transfer(block_transfer)?;
    }
    if let Some(network_transfer) = network_transfer {
        startup = startup.add_transfer(network_transfer)?;
    }
    if let Some(display_transfer) = display_transfer {
        startup = startup.add_transfer(display_transfer)?;
    }
    if let Some(input_transfer) = input_transfer {
        startup = startup.add_transfer(input_transfer)?;
    }
    if let Some(audio_transfer) = audio_transfer {
        startup = startup.add_transfer(audio_transfer)?;
    }
    ipc_kernel.send(
        bootstrap_task.capability_space(),
        kernel_bootstrap_handle,
        startup,
    )?;
    let root_task = root
        .task
        .task()
        .ok_or(BootstrapError::MissingRootTask)?
        .id();
    let root_thread = root
        .thread
        .thread()
        .ok_or(BootstrapError::MissingRootThread)?
        .id();

    log_line("bootstrap", "entering userspace executor");
    let _ = kernel.tasks().scheduler().yield_current()?;
    run_userspace_executor(kernel, root_task)?;

    let scheduler_snapshot = kernel.tasks().snapshot().scheduler;
    let exit_status = kernel_user::runtime()
        .and_then(|runtime| runtime.task_exit_status(root_task))
        .unwrap_or(TaskExitStatus::Running);

    Ok(RootBootstrapSummary {
        root_task: root_task.0,
        root_thread,
        exit_status,
        scheduler_current: scheduler_snapshot.current,
        runnable_threads: scheduler_snapshot.runnable_threads,
        blocked_threads: scheduler_snapshot.blocked_threads,
        context_switches: scheduler_snapshot.context_switches,
    })
}

pub(crate) fn resolve_boot_store_image(image_id: u32) -> Option<&'static [u8]> {
    let boot_store = BOOT_STORE_IMAGE_SOURCE.get().copied()?;
    BootStore::parse(boot_store).ok()?.resolve_image(image_id)
}

fn transfer_bootstrap_object(
    bootstrap_task: &serviceos_kernel_core::task::TaskObject,
    object: Option<serviceos_kernel_core::object::KernelObjectRef>,
    rights: CapabilityRights,
) -> Result<Option<serviceos_kernel_core::capability::PreparedTransfer>, BootstrapError> {
    let Some(object) = object else {
        return Ok(None);
    };
    let handle = bootstrap_task
        .capability_space()
        .install(object, rights, None)?;
    Ok(Some(bootstrap_task.capability_space().prepare_transfer(
        handle,
        rights,
        TransferMode::Move,
    )?))
}
