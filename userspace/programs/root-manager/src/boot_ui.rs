use core::{fmt::Write, str};

use rt::{FixedLogBuffer, ServiceId};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::{
    state::{GraphStatus, MAX_SERVICE_SLOTS, ServicePhase, ServiceSlot},
    util::{find_slot_index_checked, service_name},
};

const BOOT_SURFACE_WIDTH: u32 = 620;
const BOOT_SURFACE_HEIGHT: u32 = 236;
const BOOT_SURFACE_Z: u32 = 3_000;
const BOOT_BUFFER_SLOTS: usize = 2;
const BOOT_BUFFER_BYTES: usize = BOOT_SURFACE_WIDTH as usize * BOOT_SURFACE_HEIGHT as usize * 4;
const PANEL_X: usize = 22;
const PANEL_Y: usize = 46;
const PANEL_W: usize = BOOT_SURFACE_WIDTH as usize - 44;
const PANEL_H: usize = BOOT_SURFACE_HEIGHT as usize - 68;
const BAR_X: usize = PANEL_X + 18;
const BAR_Y: usize = PANEL_Y + 86;
const BAR_W: usize = PANEL_W - 36;
const BAR_H: usize = 18;
const CHIP_Y: usize = BAR_Y + 34;
const FOOTER_Y: usize = CHIP_Y + 34;

const BG: u32 = 0x0d1420;
const PANEL: u32 = 0x172233;
const PANEL_ALT: u32 = 0x23344a;
const PANEL_LINE: u32 = 0x2c425e;
const ACCENT: u32 = 0x7cc6ff;
const ACCENT_DIM: u32 = 0x324f70;
const READY: u32 = 0x8de19d;
const WARN: u32 = 0xf2c36b;
const TEXT_PRIMARY: u32 = 0xe7f1ff;
const TEXT_SECONDARY: u32 = 0xa6b9cf;
const TEXT_MUTED: u32 = 0x7488a0;

pub(crate) struct BootUi {
    surface_handle: rt::Handle,
    buffers: Option<ui::SurfaceBuffers<BOOT_BUFFER_SLOTS>>,
    presenter: Option<ui::FirstPresentSurface>,
    active: bool,
    last_snapshot: Option<BootSnapshot>,
}

impl BootUi {
    pub(crate) const fn empty() -> Self {
        Self {
            surface_handle: rt::INVALID_HANDLE,
            buffers: None,
            presenter: None,
            active: false,
            last_snapshot: None,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct BootSnapshot {
    ready: u32,
    total: u32,
    blocked_services: u32,
    degraded_services: u32,
    pending_service: Option<ServiceId>,
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

    let Some(graphics_handle) = ready_service_handle(slots, service_count, ServiceId::Graphics)
    else {
        return Ok(());
    };

    if !boot_ui.active {
        let output = rt::graphics_output_status(graphics_handle, 0)?.unwrap_or(
            rt::GraphicsOutputStatusInfo {
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
            },
        );
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
        let buffers = ui::SurfaceBuffers::<BOOT_BUFFER_SLOTS>::new(
            surface_handle,
            BOOT_SURFACE_WIDTH,
            BOOT_SURFACE_HEIGHT,
            BOOT_SURFACE_WIDTH,
            BOOT_BUFFER_BYTES,
        )?;
        boot_ui.surface_handle = surface_handle;
        boot_ui.buffers = Some(buffers);
        boot_ui.presenter = Some(ui::FirstPresentSurface::new(surface_handle));
        boot_ui.active = true;
    }

    let snapshot = sample_boot_snapshot(slots, service_count, graph_status);
    if boot_ui.last_snapshot == Some(snapshot) {
        return Ok(());
    }

    render(boot_ui, snapshot)?;
    boot_ui.last_snapshot = Some(snapshot);
    Ok(())
}

fn sample_boot_snapshot(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    graph_status: GraphStatus,
) -> BootSnapshot {
    BootSnapshot {
        ready: ready_boot_services(slots, service_count),
        total: total_boot_services(slots, service_count).max(1),
        blocked_services: graph_status.blocked_services,
        degraded_services: graph_status.degraded_services,
        pending_service: pending_service_id(slots, service_count),
    }
}

fn render(boot_ui: &mut BootUi, snapshot: BootSnapshot) -> rt::Result<()> {
    let Some(buffers) = boot_ui.buffers.as_mut() else {
        return Err(rt::Error::NotInitialized);
    };
    let Some(presenter) = boot_ui.presenter.as_mut() else {
        return Err(rt::Error::NotInitialized);
    };
    let (slot, buffer) = buffers.advance();
    let frame = &mut buffer.as_slice_mut()[..BOOT_BUFFER_BYTES];
    let width = BOOT_SURFACE_WIDTH as usize;
    let height = BOOT_SURFACE_HEIGHT as usize;

    ui::fill_rgba8888_rect(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        width,
        height,
        0,
        0,
        width,
        height,
        BG,
    );
    ui::fill_rgba8888_rect(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        width,
        height,
        PANEL_X,
        PANEL_Y,
        PANEL_W,
        PANEL_H,
        PANEL,
    );
    ui::fill_rgba8888_rect(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        width,
        height,
        PANEL_X,
        PANEL_Y,
        PANEL_W,
        34,
        PANEL_ALT,
    );
    ui::fill_rgba8888_rect(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        width,
        height,
        PANEL_X,
        PANEL_Y + 34,
        PANEL_W,
        1,
        PANEL_LINE,
    );

    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        28,
        22,
        TEXT_MUTED,
        "SERVICEOS",
    );
    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        PANEL_X as i32 + 18,
        PANEL_Y as i32 + 11,
        TEXT_PRIMARY,
        "STARTING ESSENTIAL SERVICES",
    );

    let ready = snapshot.ready;
    let total = snapshot.total.max(1);
    let percent = ((ready as u64 * 100) / total as u64) as u32;
    let filled = ((BAR_W as u64 * ready as u64) / total as u64) as usize;

    let mut summary = FixedLogBuffer::<64>::new();
    let _ = write!(&mut summary, "{} of {} core services ready", ready, total);
    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        PANEL_X as i32 + 18,
        PANEL_Y as i32 + 54,
        TEXT_PRIMARY,
        summary.as_str(),
    );

    let mut current = FixedLogBuffer::<72>::new();
    let _ = write!(
        &mut current,
        "Current: {}",
        snapshot
            .pending_service
            .map(service_name)
            .unwrap_or("Finalizing desktop")
    );
    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        PANEL_X as i32 + 18,
        PANEL_Y as i32 + 72,
        TEXT_SECONDARY,
        current.as_str(),
    );

    ui::fill_rgba8888_rect(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        width,
        height,
        BAR_X,
        BAR_Y,
        BAR_W,
        BAR_H,
        ACCENT_DIM,
    );
    if filled != 0 {
        ui::fill_rgba8888_rect(
            frame,
            BOOT_SURFACE_WIDTH as usize,
            width,
            height,
            BAR_X,
            BAR_Y,
            filled,
            BAR_H,
            ACCENT,
        );
    }

    let mut percent_text = FixedLogBuffer::<16>::new();
    let _ = write!(&mut percent_text, "{}%", percent);
    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        (BAR_X + BAR_W - 34) as i32,
        (BAR_Y as i32) - 14,
        TEXT_SECONDARY,
        percent_text.as_str(),
    );

    draw_chip(frame, 26, CHIP_Y, 118, 22, READY, "READY", ready);
    draw_chip(
        frame,
        154,
        CHIP_Y,
        118,
        22,
        if snapshot.blocked_services == 0 {
            PANEL_LINE
        } else {
            WARN
        },
        "BLOCKED",
        snapshot.blocked_services,
    );
    draw_chip(
        frame,
        282,
        CHIP_Y,
        126,
        22,
        if snapshot.degraded_services == 0 {
            PANEL_LINE
        } else {
            WARN
        },
        "DEGRADED",
        snapshot.degraded_services,
    );

    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        PANEL_X as i32 + 18,
        FOOTER_Y as i32,
        TEXT_MUTED,
        "Preparing services, storage, session, and desktop runtime",
    );

    presenter.present(slot, BOOT_SURFACE_WIDTH, BOOT_SURFACE_HEIGHT)
}

fn draw_chip(
    frame: &mut [u8],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: u32,
    label: &str,
    value: u32,
) {
    ui::fill_rgba8888_rect(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        BOOT_SURFACE_WIDTH as usize,
        BOOT_SURFACE_HEIGHT as usize,
        x,
        y,
        width,
        height,
        color,
    );
    let mut text = FixedLogBuffer::<32>::new();
    let _ = write!(&mut text, "{} {}", label, value);
    rt::draw_text_rgba8888(
        frame,
        BOOT_SURFACE_WIDTH as usize,
        x as i32 + 10,
        y as i32 + 7,
        if color == READY || color == WARN {
            BG
        } else {
            TEXT_SECONDARY
        },
        text.as_str(),
    );
}

fn close(boot_ui: &mut BootUi) {
    if !boot_ui.active {
        return;
    }
    let _ = rt::surface_set_visibility(boot_ui.surface_handle, false);
    boot_ui.buffers = None;
    if boot_ui.surface_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(boot_ui.surface_handle);
    }
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
        .filter(|slot| {
            slot.occupied && slot.manifest.startup == serviceos_bundle::ServiceStartupMode::Eager
        })
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

fn pending_service_id(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> Option<ServiceId> {
    slots[..service_count]
        .iter()
        .find(|slot| {
            slot.occupied
                && slot.manifest.startup == serviceos_bundle::ServiceStartupMode::Eager
                && slot.phase != ServicePhase::Ready
        })
        .map(|slot| slot.manifest.service_id)
}
