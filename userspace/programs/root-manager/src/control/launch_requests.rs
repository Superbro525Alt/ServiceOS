use rt::{LogEvent, LogSeverity, ManagerStatus, ManagerTag, RawMessage, ServiceId, rights};
use serviceos_abi::IPC_MAX_WORDS;
use serviceos_bundle::BOOT_STORE_PATH_MAX;
use serviceos_userspace_runtime as rt;

use crate::{
    state::{MAX_SERVICE_SLOTS, ServiceSlot},
    util::{emit_manager_event, image_id_from_word},
};

use super::{
    launch::{
        launch_image_is_authorized, launch_is_authorized, launch_policy_allows, launch_program,
        launch_program_from_image,
    },
    lifecycle::close_message_handles,
    storage::{load_image_from_storage, unpack_path},
};

pub(super) fn handle_launch_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::LaunchReply as u32);
    reply.word_count = 1;
    let caller = slots[service_index].manifest.service_id;
    if caller != ServiceId::Shell
        && caller != ServiceId::DesktopShell
        && caller != ServiceId::Runtime
        && caller != ServiceId::Developer
    {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }

    let image_id = image_id_from_word(message.words[0]);
    if !launch_is_authorized(caller, image_id) {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }
    if !launch_policy_allows(slots, service_count, image_id) {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        let _ = emit_manager_event(
            slots,
            service_count,
            LogSeverity::Warn,
            LogEvent::SecurityLaunchDenied,
            caller,
            image_id as u32 as u64,
        );
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }

    let startup_word_count = if message.word_count > 1 {
        (message.words[1] as usize).min(IPC_MAX_WORDS.saturating_sub(2))
    } else {
        0
    };
    if (message.word_count as usize) < 2 + startup_word_count {
        reply.words[0] = ManagerStatus::Failed as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }
    let task_handle = match launch_program(
        slots,
        service_count,
        bootstrap_authority,
        caller,
        image_id,
        &message.words[2..2 + startup_word_count],
        &message.handles[..message.handle_count as usize],
        &message.handle_rights[..message.handle_count as usize],
    ) {
        Ok(task_handle) => task_handle,
        Err(rt::Error::NotFound) => {
            reply.words[0] = ManagerStatus::NotFound as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
        Err(rt::Error::PermissionDenied) => {
            reply.words[0] = ManagerStatus::Denied as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
        Err(_) => {
            reply.words[0] = ManagerStatus::Busy as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
    };

    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.handle_count = 1;
    reply.handles[0] = task_handle;
    reply.handle_rights[0] = rights::READ;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    let _ = rt::handle_close(task_handle);
    let _ = emit_manager_event(
        slots,
        service_count,
        LogSeverity::Info,
        LogEvent::ToolLaunched,
        caller,
        image_id as u32 as u64,
    );
    Ok(())
}

pub(super) fn handle_launch_image_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::LaunchImageReply as u32);
    reply.word_count = 1;
    let caller = slots[service_index].manifest.service_id;
    if !launch_image_is_authorized(caller) || message.handle_count < 1 {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }

    let startup_word_count = if message.word_count > 0 {
        (message.words[0] as usize).min(IPC_MAX_WORDS.saturating_sub(1))
    } else {
        0
    };
    if (message.word_count as usize) < 1 + startup_word_count {
        reply.words[0] = ManagerStatus::Failed as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }
    let task_handle = match launch_program_from_image(
        slots,
        service_count,
        bootstrap_authority,
        caller,
        message.handles[0],
        &message.words[1..1 + startup_word_count],
        &message.handles[1..message.handle_count as usize],
        &message.handle_rights[1..message.handle_count as usize],
    ) {
        Ok(task_handle) => task_handle,
        Err(rt::Error::PermissionDenied) => {
            reply.words[0] = ManagerStatus::Denied as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
        Err(rt::Error::NotFound) => {
            reply.words[0] = ManagerStatus::NotFound as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
        Err(_) => {
            reply.words[0] = ManagerStatus::Busy as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
    };

    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.handle_count = 1;
    reply.handles[0] = task_handle;
    reply.handle_rights[0] = rights::READ;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    let _ = rt::handle_close(task_handle);
    let _ = emit_manager_event(
        slots,
        service_count,
        LogSeverity::Info,
        LogEvent::ToolLaunched,
        caller,
        0xffff_ffff,
    );
    Ok(())
}

pub(super) fn handle_launch_stored_image_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::LaunchStoredImageReply as u32);
    reply.word_count = 1;
    let caller = slots[service_index].manifest.service_id;
    if !launch_image_is_authorized(caller) || message.word_count < 2 {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        close_message_handles(message);
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let startup_word_count = (message.words[0] as usize).min(IPC_MAX_WORDS.saturating_sub(2));
    let path_len = message.words[1] as usize;
    if (message.word_count as usize) < 2 + startup_word_count {
        reply.words[0] = ManagerStatus::Failed as u32 as u64;
        close_message_handles(message);
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let mut path_bytes = [0u8; BOOT_STORE_PATH_MAX];
    let path = unpack_path(
        &message.words[2 + startup_word_count..message.word_count as usize],
        path_len,
        &mut path_bytes,
    )?;

    let image_handle = match load_image_from_storage(slots, service_count, path) {
        Ok(handle) => handle,
        Err(rt::Error::NotFound) => {
            reply.words[0] = ManagerStatus::NotFound as u32 as u64;
            close_message_handles(message);
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
        Err(error) => {
            let _ = crate::util::fallback_logf(format_args!(
                "launch stored image load failed path={} error={:?}",
                path, error
            ));
            reply.words[0] = ManagerStatus::Busy as u32 as u64;
            close_message_handles(message);
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
    };

    let task_handle = match launch_program_from_image(
        slots,
        service_count,
        bootstrap_authority,
        caller,
        image_handle,
        &message.words[2..2 + startup_word_count],
        &message.handles[..message.handle_count as usize],
        &message.handle_rights[..message.handle_count as usize],
    ) {
        Ok(task_handle) => task_handle,
        Err(rt::Error::PermissionDenied) => {
            let _ = rt::handle_close(image_handle);
            reply.words[0] = ManagerStatus::Denied as u32 as u64;
            close_message_handles(message);
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
        Err(error) => {
            let _ = crate::util::fallback_logf(format_args!(
                "launch stored image spawn failed path={} error={:?}",
                path, error
            ));
            let _ = rt::handle_close(image_handle);
            reply.words[0] = ManagerStatus::Busy as u32 as u64;
            close_message_handles(message);
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
    };

    let _ = rt::handle_close(image_handle);
    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.handle_count = 1;
    reply.handles[0] = task_handle;
    reply.handle_rights[0] = rights::READ;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    let _ = rt::handle_close(task_handle);
    Ok(())
}
