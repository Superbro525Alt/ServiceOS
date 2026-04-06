use core::{fmt::Write, str};

use serviceos_userspace_runtime as rt;
use rt::{FixedLogBuffer, ServiceId};

use crate::{
    state::{GraphStatus, ServicePhase, ServiceSlot, MAX_SERVICE_SLOTS},
    util::{find_slot_index_checked, service_name},
};

const BOOT_SURFACE_WIDTH: u32 = 520;
const BOOT_SURFACE_HEIGHT: u32 = 180;
const BOOT_SURFACE_Z: u32 = 3_000;
const BG: u32 = 0x111821;
const PANEL: u32 = 0x1a2432;
const PANEL_ALT: u32 = 0x223247;
const ACCENT: u32 = 0x7cc6ff;
const ACCENT_DIM: u32 = 0x36506b;
const TEXT_PRIMARY: u32 = 0xe7f1ff;
const TEXT_SECONDARY: u32 = 0xa6b9cf;
const TEXT_WARN: u32 = 0xf2c36b;

#[derive(Clone, Copy)]
pub(crate) struct BootUi {
    surface_handle: rt::Handle,
    active: bool,
}

impl BootUi {
    pub(crate) const fn empty() -> Self {
        Self {
            surface_handle: rt::INVALID_HANDLE,
            active: false,
        }
    }
}

pub(crate) fn update(
    boot_ui: &mut BootUi,
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    graph_status: GraphStatus,
) -> rt::Result<()> {
    if desktop_ready(slots, service_count) {
        close(boot_ui);
        return Ok(());
    }

    let Some(graphics_handle) = ready_service_handle(slots, service_count, ServiceId::Graphics) else {
        return Ok(());
    };

    if !boot_ui.active {
        let output = rt::graphics_output_status(graphics_handle, 0)?.unwrap_or(rt::GraphicsOutputStatusInfo {
            index: 0,
            backend: rt::DisplayOutputBackend::Unknown,
            state: rt::DisplayOutputState::Connected,
            pixel_format: rt::DisplayPixelFormat::Unknown,
            width: 1280,
            height: 800,
            stride: 1280,
            bytes_per_pixel: 4,
            byte_len: 0,
            present_count: 0,
            surface_count: 0,
        });
        let x = ((output.width.saturating_sub(BOOT_SURFACE_WIDTH)) / 2) as i32;
        let y = ((output.height.saturating_sub(BOOT_SURFACE_HEIGHT)) / 2) as i32;
        let (_, surface_handle) = rt::graphics_surface_create(
            graphics_handle,
            0,
            x,
            y,
            BOOT_SURFACE_WIDTH,
            BOOT_SURFACE_HEIGHT,
            BOOT_SURFACE_Z,
            BG,
            false,
        )?;
        boot_ui.surface_handle = surface_handle;
        boot_ui.active = true;
    }

    render(boot_ui.surface_handle, slots, service_count, graph_status)?;
    Ok(())
}

fn render(
    surface_handle: rt::Handle,
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    graph_status: GraphStatus,
) -> rt::Result<()> {
    let total = total_boot_services(slots, service_count).max(1);
    let ready = ready_boot_services(slots, service_count);
    let progress_width = 472u32;
    let filled = ((progress_width as u64 * ready as u64) / total as u64) as u32;

    let mut summary = FixedLogBuffer::<64>::new();
    let _ = write!(&mut summary, "{} / {} essential services ready", ready, total);
    let mut pending = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut pending,
        "CURRENT {}",
        pending_service_name(slots, service_count)
    );
    let mut detail = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut detail,
        "blocked={} degraded={}",
        graph_status.blocked_services,
        graph_status.degraded_services
    );

    rt::surface_set_fill(surface_handle, BG)?;
    rt::surface_clear_scene(surface_handle)?;
    rt::surface_set_rect(surface_handle, 0, 0, 0, BOOT_SURFACE_WIDTH, BOOT_SURFACE_HEIGHT, PANEL, true)?;
    rt::surface_set_rect(surface_handle, 1, 0, 0, BOOT_SURFACE_WIDTH, 30, PANEL_ALT, true)?;
    rt::surface_set_label(surface_handle, 0, 18, 10, TEXT_PRIMARY, "SERVICEOS STARTING")?;
    rt::surface_set_label(
        surface_handle,
        1,
        20,
        52,
        TEXT_PRIMARY,
        str::from_utf8(summary.as_bytes()).unwrap_or("starting"),
    )?;
    rt::surface_set_label(
        surface_handle,
        2,
        20,
        72,
        TEXT_SECONDARY,
        str::from_utf8(pending.as_bytes()).unwrap_or("current"),
    )?;
    rt::surface_set_label(
        surface_handle,
        3,
        20,
        92,
        if graph_status.degraded_services != 0 || graph_status.blocked_services != 0 {
            TEXT_WARN
        } else {
            TEXT_SECONDARY
        },
        str::from_utf8(detail.as_bytes()).unwrap_or("status"),
    )?;
    rt::surface_set_rect(surface_handle, 4, 20, 122, progress_width, 18, ACCENT_DIM, true)?;
    rt::surface_set_rect(surface_handle, 5, 20, 122, filled.max(6).min(progress_width), 18, ACCENT, true)?;
    rt::surface_set_label(surface_handle, 6, 20, 150, TEXT_SECONDARY, "Loading core services and desktop runtime")?;
    rt::surface_set_visibility(surface_handle, true)
}

fn close(boot_ui: &mut BootUi) {
    if !boot_ui.active {
        return;
    }
    let _ = rt::surface_set_visibility(boot_ui.surface_handle, false);
    let _ = rt::handle_close(boot_ui.surface_handle);
    *boot_ui = BootUi::empty();
}

fn ready_service_handle(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
) -> Option<rt::Handle> {
    let index = find_slot_index_checked(slots, service_count, service_id)?;
    let slot = &slots[index];
    if slot.phase == ServicePhase::Ready && slot.public_handle != rt::INVALID_HANDLE {
        Some(slot.public_handle)
    } else {
        None
    }
}

fn desktop_ready(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> bool {
    find_slot_index_checked(slots, service_count, ServiceId::DesktopShell)
        .map(|index| slots[index].phase == ServicePhase::Ready)
        .unwrap_or(false)
}

fn total_boot_services(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> u32 {
    slots[..service_count]
        .iter()
        .filter(|slot| slot.occupied && slot.manifest.startup == serviceos_bundle::ServiceStartupMode::Eager)
        .count() as u32
}

fn ready_boot_services(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> u32 {
    slots[..service_count]
        .iter()
        .filter(|slot| {
            slot.occupied
                && slot.manifest.startup == serviceos_bundle::ServiceStartupMode::Eager
                && slot.phase == ServicePhase::Ready
        })
        .count() as u32
}

fn pending_service_name(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> &'static str {
    slots[..service_count]
        .iter()
        .find(|slot| {
            slot.occupied
                && slot.manifest.startup == serviceos_bundle::ServiceStartupMode::Eager
                && slot.phase != ServicePhase::Ready
        })
        .map(|slot| service_name(slot.manifest.service_id))
        .unwrap_or("FINALIZING")
}
