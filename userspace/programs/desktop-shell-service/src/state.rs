use serviceos_userspace_runtime as rt;
use rt::{DesktopAppId, ServiceImageId};
use serviceos_desktop_ui as ui;

pub(crate) const SESSION_ID: u32 = 1;
pub(crate) const APP_COUNT: usize = 5;
pub(crate) const APP_PAGE_SIZE: usize = 4;
pub(crate) const WINDOW_PAGE_SIZE: usize = 2;
pub(crate) const WORKSPACE_COUNT: u32 = 4;
pub(crate) const MAX_DESKTOP_REQUESTS_PER_TURN: usize = 24;
pub(crate) const APP_REFRESH_TICKS: u64 = 10;
pub(crate) const STATUS_REFRESH_TICKS: u64 = 300;
pub(crate) const TOPBAR_HEIGHT: u32 = 42;
pub(crate) const LAUNCHER_WIDTH: u32 = 250;
pub(crate) const LAUNCHER_HEIGHT: u32 = 144;
pub(crate) const PANEL_MARGIN: u32 = 20;
pub(crate) const STATUS_PANEL_WIDTH: u32 = 280;
pub(crate) const STATUS_PANEL_HEIGHT: u32 = 160;
pub(crate) const WINDOW_MIN_WIDTH: u32 = 280;
pub(crate) const WINDOW_MIN_HEIGHT: u32 = 160;
pub(crate) const RESIZE_GRIP_SIZE: i32 = 20;
pub(crate) const CURSOR_SIZE: u32 = 14;
pub(crate) const CURSOR_Z_ORDER: u32 = 4_096;
pub(crate) const NOTIFICATION_TIMEOUT_TICKS: u64 = 300;
pub(crate) const MAX_NOTIFICATION_BYTES: usize = 96;
pub(crate) const NOTIFICATION_HISTORY_MAX: usize = 12;
pub(crate) const NOTIFICATION_HISTORY_TEXT_MAX: usize = 64;
pub(crate) const CLIPBOARD_HISTORY_LINES: usize = 5;
pub(crate) const PALETTE_QUERY_MAX: usize = 32;
pub(crate) const OVERLAY_RESULT_MAX: usize = 6;
pub(crate) const SWITCHER_WIDTH: u32 = 280;
pub(crate) const SWITCHER_HEIGHT: u32 = 132;
pub(crate) const PALETTE_WIDTH: u32 = 360;
pub(crate) const PALETTE_HEIGHT: u32 = 188;
pub(crate) const PALETTE_BUFFER_SLOTS: usize = 2;
pub(crate) const PALETTE_BUFFER_BYTES: usize = PALETTE_WIDTH as usize * PALETTE_HEIGHT as usize * 4;
pub(crate) const HISTORY_WIDTH: u32 = 360;
pub(crate) const HISTORY_HEIGHT: u32 = 188;
pub(crate) const MOD_SHIFT: u32 = 1 << 0;
pub(crate) const MOD_ALT: u32 = 1 << 1;
pub(crate) const MOD_CTRL: u32 = 1 << 2;
pub(crate) const KEY_ESC: u32 = 1;
pub(crate) const KEY_TAB: u32 = 15;
pub(crate) const KEY_BACKSPACE: u32 = 14;
pub(crate) const KEY_ENTER: u32 = 28;
pub(crate) const KEY_SPACE: u32 = 57;
pub(crate) const KEY_V: u32 = 47;
pub(crate) const KEY_N: u32 = 49;
pub(crate) const KEY_UP: u32 = 103;
pub(crate) const KEY_DOWN: u32 = 108;
pub(crate) const KEY_LEFT_ALT: u32 = 56;
pub(crate) const KEY_RIGHT_ALT: u32 = 100;
pub(crate) const KEY_F4: u32 = 62;
pub(crate) const KEY_1: u32 = 2;
pub(crate) const KEY_2: u32 = 3;
pub(crate) const KEY_3: u32 = 4;
pub(crate) const KEY_4: u32 = 5;
pub(crate) const KEY_5: u32 = 6;

#[derive(Clone, Copy)]
pub(crate) struct Chrome {
    pub(crate) desktop_handle: rt::Handle,
    pub(crate) topbar_handle: rt::Handle,
    pub(crate) launcher_handle: rt::Handle,
    pub(crate) status_handle: rt::Handle,
    pub(crate) switcher_handle: rt::Handle,
    pub(crate) palette_handle: rt::Handle,
    pub(crate) notifications_handle: rt::Handle,
    pub(crate) clipboard_handle: rt::Handle,
    pub(crate) cursor_handle: rt::Handle,
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct WindowState {
    pub(crate) surface_id: u32,
    pub(crate) surface_handle: rt::Handle,
    pub(crate) control_handle: rt::Handle,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) z_order: u32,
    pub(crate) minimized: bool,
    pub(crate) maximized: bool,
    pub(crate) restore_x: i32,
    pub(crate) restore_y: i32,
    pub(crate) restore_width: u32,
    pub(crate) restore_height: u32,
}

impl WindowState {
    pub(crate) const fn empty() -> Self {
        Self {
            surface_id: 0,
            surface_handle: rt::INVALID_HANDLE,
            control_handle: rt::INVALID_HANDLE,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            z_order: 0,
            minimized: false,
            maximized: false,
            restore_x: 0,
            restore_y: 0,
            restore_width: 0,
            restore_height: 0,
        }
    }

    pub(crate) fn visible(&self) -> bool {
        self.surface_id != 0 && !self.minimized
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AppSlot {
    pub(crate) app_id: DesktopAppId,
    pub(crate) image_id: ServiceImageId,
    pub(crate) task_handle: rt::Handle,
    pub(crate) window: WindowState,
    pub(crate) running: bool,
    pub(crate) workspace_id: u32,
    pub(crate) launch_count: u32,
}

impl AppSlot {
    pub(crate) const fn new(app_id: DesktopAppId, image_id: ServiceImageId) -> Self {
        Self {
            app_id,
            image_id,
            task_handle: rt::INVALID_HANDLE,
            window: WindowState::empty(),
            running: false,
            workspace_id: 1,
            launch_count: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DesktopStatusSnapshot {
    pub(crate) running_apps: u32,
    pub(crate) focused_app: Option<DesktopAppId>,
    pub(crate) active_workspace: u32,
    pub(crate) tracked_services: u64,
    pub(crate) ipv4_address: u32,
    pub(crate) notification_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OverlayMode {
    None,
    Switcher,
    CommandPalette,
    Notifications,
    ClipboardHistory,
}

#[derive(Clone, Copy)]
pub(crate) struct NotificationEntry {
    pub(crate) occupied: bool,
    pub(crate) sequence: u32,
    pub(crate) source_app: Option<DesktopAppId>,
    pub(crate) actionable: bool,
    pub(crate) text_len: usize,
    pub(crate) text: [u8; NOTIFICATION_HISTORY_TEXT_MAX],
}

impl NotificationEntry {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            sequence: 0,
            source_app: None,
            actionable: false,
            text_len: 0,
            text: [0; NOTIFICATION_HISTORY_TEXT_MAX],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DragState {
    Move {
        app_id: DesktopAppId,
        grab_offset_x: i32,
        grab_offset_y: i32,
    },
    Resize {
        app_id: DesktopAppId,
        edges: ResizeEdges,
        origin_pointer_x: i32,
        origin_pointer_y: i32,
        start_x: i32,
        start_y: i32,
        start_width: u32,
        start_height: u32,
    },
}

impl DragState {
    pub(crate) fn mode(self) -> rt::DesktopDragMode {
        match self {
            Self::Move { .. } => rt::DesktopDragMode::Move,
            Self::Resize { .. } => rt::DesktopDragMode::Resize,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResizeEdges(pub(crate) u8);

impl ResizeEdges {
    pub(crate) const NONE: Self = Self(0);
    pub(crate) const LEFT: Self = Self(1 << 0);
    pub(crate) const RIGHT: Self = Self(1 << 1);
    pub(crate) const TOP: Self = Self(1 << 2);
    pub(crate) const BOTTOM: Self = Self(1 << 3);

    pub(crate) fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOrAssign for ResizeEdges {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

pub(crate) enum HitTarget {
    Background,
    Launcher(DesktopAppId),
    WindowContent(DesktopAppId),
    WindowMove {
        app_id: DesktopAppId,
        grab_offset_x: i32,
        grab_offset_y: i32,
    },
    WindowResize {
        app_id: DesktopAppId,
        edges: ResizeEdges,
    },
    WindowClose(DesktopAppId),
    WindowMinimize(DesktopAppId),
    WindowMaximize(DesktopAppId),
}

#[derive(Clone, Copy)]
pub(crate) struct ContentCapture {
    pub(crate) app_id: DesktopAppId,
    pub(crate) button: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PendingResize {
    pub(crate) app_id: DesktopAppId,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct DesktopState {
    pub(crate) bootstrap: rt::Handle,
    pub(crate) log_handle: rt::Handle,
    pub(crate) graphics_handle: rt::Handle,
    pub(crate) session_handle: rt::Handle,
    pub(crate) network_handle: rt::Handle,
    pub(crate) system_status_handle: rt::Handle,
    pub(crate) clipboard_service_handle: rt::Handle,
    pub(crate) chrome: Chrome,
    pub(crate) palette_buffers: ui::SurfaceBuffers<PALETTE_BUFFER_SLOTS>,
    pub(crate) palette_presenter: ui::FirstPresentSurface,
    pub(crate) apps: [AppSlot; APP_COUNT],
    pub(crate) focused_app: Option<DesktopAppId>,
    pub(crate) active_workspace: u32,
    pub(crate) recent_focus: [DesktopAppId; APP_COUNT],
    pub(crate) recent_focus_len: usize,
    pub(crate) next_app_refresh: u64,
    pub(crate) next_status_refresh: u64,
    pub(crate) last_status_snapshot: Option<DesktopStatusSnapshot>,
    pub(crate) pending_shell_refresh: rt::PendingFlag,
    pub(crate) pending_focus_refresh: rt::PendingFlag,
    pub(crate) pending_app_launch: rt::PendingValue<DesktopAppId>,
    pub(crate) next_z_order: u32,
    pub(crate) pointer_x: i32,
    pub(crate) pointer_y: i32,
    pub(crate) drag_state: Option<DragState>,
    pub(crate) content_capture: Option<ContentCapture>,
    pub(crate) pending_resize: Option<PendingResize>,
    pub(crate) notification: [u8; MAX_NOTIFICATION_BYTES],
    pub(crate) notification_len: usize,
    pub(crate) notification_deadline: u64,
    pub(crate) notification_history: [NotificationEntry; NOTIFICATION_HISTORY_MAX],
    pub(crate) notification_history_len: usize,
    pub(crate) next_notification_sequence: u32,
    pub(crate) overlay_mode: OverlayMode,
    pub(crate) overlay_selection: usize,
    pub(crate) palette_query: [u8; PALETTE_QUERY_MAX],
    pub(crate) palette_query_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PaletteAction {
    Launch(DesktopAppId),
    ShowNotifications,
    ShowClipboardHistory,
    SwitchWorkspace(u32),
    FocusNext,
}
