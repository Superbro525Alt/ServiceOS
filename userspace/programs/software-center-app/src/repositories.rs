use core::fmt::Write as _;

use rt::{PackageChannel, PackageRepositorySyncState, PackageRepositoryTrustMode, PackageRing};
use serviceos_userspace_runtime as rt;

use crate::actions::{error_label, set_statusf};
use crate::state::{AppState, Layout, BUTTON_HEIGHT, ROW_HEIGHT};

/// Package-service keeps at most 4 repository slots (`MAX_REPOSITORIES`).
pub(crate) const MAX_REPOS: usize = 4;
/// Mirrors the shell's `MAX_PACKAGE_TEXT`; names and URLs ride one packed
/// byte blob, so both buffers must fit any entry.
pub(crate) const MAX_REPO_TEXT: usize = 96;

pub(crate) const MAX_NAME_BYTES: usize = 40;
pub(crate) const MAX_URL_BYTES: usize = 64;
pub(crate) const MAX_DIGEST_BYTES: usize = 16;

#[derive(Clone, Copy)]
pub(crate) struct RepoEntry {
    pub(crate) info: rt::PackageRepositoryInfo,
    pub(crate) name: [u8; MAX_REPO_TEXT],
    pub(crate) url: [u8; MAX_REPO_TEXT],
}

impl RepoEntry {
    pub(crate) const fn empty() -> Self {
        Self {
            info: rt::PackageRepositoryInfo {
                repo_index: 0,
                package_count: 0,
                trust_mode: PackageRepositoryTrustMode::Boot,
                sync_state: PackageRepositorySyncState::Idle,
                channel: PackageChannel::Stable,
                ring: PackageRing::Production,
                enabled: false,
                pinned_digest: 0,
                last_digest: 0,
                name_len: 0,
                url_len: 0,
            },
            name: [0; MAX_REPO_TEXT],
            url: [0; MAX_REPO_TEXT],
        }
    }

    pub(crate) fn name_text(&self) -> &str {
        core::str::from_utf8(&self.name[..self.info.name_len]).unwrap_or("?")
    }

    pub(crate) fn url_text(&self) -> &str {
        core::str::from_utf8(&self.url[..self.info.url_len]).unwrap_or("?")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourcesPhase {
    /// Browsing sources + add form.
    Form,
    /// Two-phase trust review for a pending add.
    Review,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum AddField {
    Name,
    Url,
    Digest,
}

#[derive(Clone, Copy)]
pub(crate) struct SourcesState {
    pub(crate) open: bool,
    /// False after a transport failure: the panel renders an honest
    /// "unavailable" notice instead of pretending to show sources.
    pub(crate) available: bool,
    pub(crate) repos: [RepoEntry; MAX_REPOS],
    pub(crate) repo_count: usize,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
    pub(crate) phase: SourcesPhase,
    pub(crate) field: AddField,
    pub(crate) name: [u8; MAX_NAME_BYTES],
    pub(crate) name_len: usize,
    pub(crate) url: [u8; MAX_URL_BYTES],
    pub(crate) url_len: usize,
    pub(crate) trust: PackageRepositoryTrustMode,
    pub(crate) digest: [u8; MAX_DIGEST_BYTES],
    pub(crate) digest_len: usize,
}

impl SourcesState {
    pub(crate) const fn new() -> Self {
        Self {
            open: false,
            available: true,
            repos: [RepoEntry::empty(); MAX_REPOS],
            repo_count: 0,
            selected: 0,
            scroll: 0,
            phase: SourcesPhase::Form,
            field: AddField::Name,
            name: [0; MAX_NAME_BYTES],
            name_len: 0,
            url: [0; MAX_URL_BYTES],
            url_len: 0,
            trust: PackageRepositoryTrustMode::Unsigned,
            digest: [0; MAX_DIGEST_BYTES],
            digest_len: 0,
        }
    }

    pub(crate) fn selected_repo(&self) -> Option<&RepoEntry> {
        if self.repo_count == 0 {
            return None;
        }
        self.repos.get(self.selected)
    }

    pub(crate) fn move_selection(&mut self, step: i32) {
        if self.repo_count == 0 {
            return;
        }
        let next = self.selected as i32 + step;
        self.selected = next.clamp(0, self.repo_count as i32 - 1) as usize;
    }

    pub(crate) fn ensure_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    pub(crate) fn cycle_trust(&mut self) {
        self.trust = match self.trust {
            PackageRepositoryTrustMode::Unsigned => PackageRepositoryTrustMode::PinnedDigest,
            PackageRepositoryTrustMode::PinnedDigest => PackageRepositoryTrustMode::SignedKey,
            PackageRepositoryTrustMode::SignedKey => PackageRepositoryTrustMode::Boot,
            PackageRepositoryTrustMode::Boot => PackageRepositoryTrustMode::Unsigned,
        };
        if self.trust != PackageRepositoryTrustMode::PinnedDigest {
            self.digest_len = 0;
            self.digest = [0; MAX_DIGEST_BYTES];
        }
    }

    pub(crate) fn push_field_char(&mut self, byte: u8) -> bool {
        if !field_char_ok(self.field, byte) {
            return false;
        }
        let (buffer, len) = match self.field {
            AddField::Name => (&mut self.name as &mut [u8], &mut self.name_len),
            AddField::Url => (&mut self.url as &mut [u8], &mut self.url_len),
            AddField::Digest => (&mut self.digest as &mut [u8], &mut self.digest_len),
        };
        if *len >= buffer.len() {
            return false;
        }
        buffer[*len] = byte;
        *len += 1;
        true
    }

    pub(crate) fn pop_field_char(&mut self) -> bool {
        let (buffer, len) = match self.field {
            AddField::Name => (&mut self.name as &mut [u8], &mut self.name_len),
            AddField::Url => (&mut self.url as &mut [u8], &mut self.url_len),
            AddField::Digest => (&mut self.digest as &mut [u8], &mut self.digest_len),
        };
        if *len == 0 {
            return false;
        }
        *len -= 1;
        buffer[*len] = 0;
        true
    }

    pub(crate) fn field_text(&self, field: AddField) -> &str {
        match field {
            AddField::Name => core::str::from_utf8(&self.name[..self.name_len]).unwrap_or(""),
            AddField::Url => core::str::from_utf8(&self.url[..self.url_len]).unwrap_or(""),
            AddField::Digest => core::str::from_utf8(&self.digest[..self.digest_len]).unwrap_or(""),
        }
    }

    /// Two-phase step 1: promote a valid form to the trust-review panel.
    pub(crate) fn begin_review(&mut self) -> bool {
        if self.phase == SourcesPhase::Form && self.form_valid() {
            self.phase = SourcesPhase::Review;
            return true;
        }
        false
    }

    pub(crate) fn cancel_review(&mut self) {
        self.phase = SourcesPhase::Form;
    }

    pub(crate) fn in_review(&self) -> bool {
        self.phase == SourcesPhase::Review
    }

    pub(crate) fn form_valid(&self) -> bool {
        if self.name_len == 0 || self.url_len == 0 {
            return false;
        }
        if self.trust == PackageRepositoryTrustMode::PinnedDigest {
            return self.parse_digest().is_some();
        }
        true
    }

    pub(crate) fn parse_digest(&self) -> Option<u64> {
        let text = core::str::from_utf8(&self.digest[..self.digest_len]).ok()?;
        let trimmed = text.strip_prefix("0x").unwrap_or(text);
        if trimmed.is_empty() {
            return None;
        }
        u64::from_str_radix(trimmed, 16).ok()
    }

    pub(crate) fn reset_form(&mut self) {
        self.name_len = 0;
        self.name = [0; MAX_NAME_BYTES];
        self.url_len = 0;
        self.url = [0; MAX_URL_BYTES];
        self.digest_len = 0;
        self.digest = [0; MAX_DIGEST_BYTES];
        self.trust = PackageRepositoryTrustMode::Unsigned;
        self.field = AddField::Name;
    }

    fn copy_name(&self) -> ([u8; MAX_NAME_BYTES], usize) {
        let mut buffer = [0u8; MAX_NAME_BYTES];
        buffer[..self.name_len].copy_from_slice(&self.name[..self.name_len]);
        (buffer, self.name_len)
    }

    fn copy_url(&self) -> ([u8; MAX_URL_BYTES], usize) {
        let mut buffer = [0u8; MAX_URL_BYTES];
        buffer[..self.url_len].copy_from_slice(&self.url[..self.url_len]);
        (buffer, self.url_len)
    }
}

fn field_char_ok(field: AddField, byte: u8) -> bool {
    match field {
        AddField::Name => byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'),
        AddField::Url => {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'_' | b'.' | b'/' | b':' | b'%' | b'?' | b'=' | b'&'
                )
        }
        AddField::Digest => byte.is_ascii_hexdigit(),
    }
}

/// Shell-parity label strings (`shell-service/src/commands/package/parse.rs`).
pub(crate) fn trust_mode_name(value: PackageRepositoryTrustMode) -> &'static str {
    match value {
        PackageRepositoryTrustMode::Boot => "boot",
        PackageRepositoryTrustMode::Unsigned => "unsigned",
        PackageRepositoryTrustMode::PinnedDigest => "pinned",
        PackageRepositoryTrustMode::SignedKey => "signed-key",
    }
}

pub(crate) fn sync_state_name(value: PackageRepositorySyncState) -> &'static str {
    match value {
        PackageRepositorySyncState::Idle => "idle",
        PackageRepositorySyncState::Ready => "ready",
        PackageRepositorySyncState::Offline => "offline",
        PackageRepositorySyncState::Failed => "failed",
    }
}

pub(crate) fn repo_channel_name(value: PackageChannel) -> &'static str {
    match value {
        PackageChannel::Stable => "stable",
        PackageChannel::Beta => "beta",
        PackageChannel::Canary => "canary",
    }
}

pub(crate) fn repo_ring_name(value: PackageRing) -> &'static str {
    match value {
        PackageRing::Production => "production",
        PackageRing::Preview => "preview",
        PackageRing::Testing => "testing",
    }
}

/// Verbatim mirrors of the shell trust-review strings
/// (`shell-service/src/commands/package/onboard.rs`).
pub(crate) fn trust_meaning(mode: PackageRepositoryTrustMode) -> &'static str {
    match mode {
        PackageRepositoryTrustMode::Boot => "packages verify against the boot trust root",
        PackageRepositoryTrustMode::Unsigned => {
            "no signature evidence; package bytes are trusted as-fetched"
        }
        PackageRepositoryTrustMode::PinnedDigest => {
            "feed digest must equal your pinned digest on every sync"
        }
        PackageRepositoryTrustMode::SignedKey => {
            "feed must verify against the source key bound at repo add time"
        }
    }
}

pub(crate) fn trust_onboarding_impact(mode: PackageRepositoryTrustMode) -> &'static str {
    match mode {
        PackageRepositoryTrustMode::Boot => {
            "installs from this source run without per-install acknowledgement"
        }
        PackageRepositoryTrustMode::Unsigned => {
            "every install from this source needs --yes and is flagged unverified"
        }
        PackageRepositoryTrustMode::PinnedDigest => {
            "sync fails closed when the fetched digest differs from the pin"
        }
        PackageRepositoryTrustMode::SignedKey => {
            "sync fails closed unless the feed verifies under the bound active ed25519 key"
        }
    }
}

/// Request packing parity for `package_repository_add`: word 0 carries the
/// packed trust/channel/ring/enabled flags, word 1 the pinned digest. GUI
/// adds ship as stable/production/enabled. The live request is packed inside
/// the runtime helper; this mirror pins the contract in host tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_add_words(trust: PackageRepositoryTrustMode, digest: u64) -> (u64, u64) {
    let word0 = (trust as u64)
        | ((PackageChannel::Stable as u64) << 16)
        | ((PackageRing::Production as u64) << 32)
        | (1u64 << 48);
    (word0, digest)
}

/// Click routing for the sources view, decided purely from state + layout so
/// it stays host-testable. Handle-needing effects run in `control.rs`.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SourcesClick {
    None,
    SelectRepo(usize),
    Field(AddField),
    CycleTrust,
    BeginReview,
    ConfirmAdd,
    CancelReview,
    SyncThis,
}

pub(crate) struct SourcesRects {
    pub(crate) field_x0: i32,
    pub(crate) field_x1: i32,
    pub(crate) name_y: i32,
    pub(crate) url_y: i32,
    pub(crate) trust_y: i32,
    pub(crate) digest_y: i32,
    pub(crate) review_y0: i32,
    pub(crate) review_y1: i32,
    pub(crate) primary_x0: i32,
    pub(crate) primary_x1: i32,
    pub(crate) secondary_x0: i32,
    pub(crate) secondary_x1: i32,
    pub(crate) button_y0: i32,
    pub(crate) button_y1: i32,
}

/// Geometry for the right-panel form/review block. `pinned` widens the stack
/// to include the digest field and pushes the action row down to match.
pub(crate) fn rects(layout: Layout, pinned: bool) -> SourcesRects {
    let field_x0 = layout.right_x + 12;
    let field_x1 = layout.right_x + layout.right_w - 12;
    let name_y = layout.detail_body_y + 76;
    let url_y = name_y + 24;
    let trust_y = url_y + 24;
    let digest_y = trust_y + 24;
    let button_y0 = if pinned { digest_y + 28 } else { trust_y + 28 };
    let button_y1 = button_y0 + BUTTON_HEIGHT;
    let button_w = 128;
    let primary_x0 = field_x0;
    let primary_x1 = primary_x0 + button_w;
    let secondary_x1 = field_x1;
    let secondary_x0 = secondary_x1 - button_w;
    SourcesRects {
        field_x0,
        field_x1,
        name_y,
        url_y,
        trust_y,
        digest_y,
        review_y0: layout.detail_body_y + 8,
        review_y1: button_y0 - 10,
        primary_x0,
        primary_x1,
        secondary_x0,
        secondary_x1,
        button_y0,
        button_y1,
    }
}

fn inside(x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
    x >= x0 && x < x1 && y >= y0 && y < y1
}

pub(crate) fn handle_pointer(state: &SourcesState, layout: Layout, x: i32, y: i32) -> SourcesClick {
    // Left-panel repo rows share the catalog list geometry.
    if x >= layout.left_x + 8 && x < layout.left_x + layout.left_w - 8 && y >= layout.list_rows_y {
        let visible = layout.visible_rows();
        let row = ((y - layout.list_rows_y) / ROW_HEIGHT) as usize;
        let position = state.scroll + row;
        if row < visible && position < state.repo_count {
            return SourcesClick::SelectRepo(position);
        }
    }

    let area = rects(
        layout,
        state.trust == PackageRepositoryTrustMode::PinnedDigest,
    );
    if state.phase == SourcesPhase::Review {
        if inside(
            x,
            y,
            area.primary_x0,
            area.button_y0,
            area.primary_x1,
            area.button_y1,
        ) {
            return SourcesClick::ConfirmAdd;
        }
        if inside(
            x,
            y,
            area.secondary_x0,
            area.button_y0,
            area.secondary_x1,
            area.button_y1,
        ) {
            return SourcesClick::CancelReview;
        }
        return SourcesClick::None;
    }

    let field_row =
        |top: i32| -> bool { inside(x, y, area.field_x0, top, area.field_x1, top + 20) };
    if field_row(area.name_y) {
        return SourcesClick::Field(AddField::Name);
    }
    if field_row(area.url_y) {
        return SourcesClick::Field(AddField::Url);
    }
    if field_row(area.trust_y) {
        return SourcesClick::CycleTrust;
    }
    if state.trust == PackageRepositoryTrustMode::PinnedDigest && field_row(area.digest_y) {
        return SourcesClick::Field(AddField::Digest);
    }
    if inside(
        x,
        y,
        area.primary_x0,
        area.button_y0,
        area.primary_x1,
        area.button_y1,
    ) {
        return SourcesClick::BeginReview;
    }
    if inside(
        x,
        y,
        area.secondary_x0,
        area.button_y0,
        area.secondary_x1,
        area.button_y1,
    ) {
        return SourcesClick::SyncThis;
    }
    SourcesClick::None
}

/// Key routing for the sources view. Printable characters are routed by the
/// caller (`keycode_to_char`) straight into the focused form field.
pub(crate) enum SourcesKey {
    None,
    BeginReview,
    ConfirmAdd,
    /// Cancel the review, or close the whole sources view from the form.
    Back,
}

pub(crate) fn handle_key(state: &mut SourcesState, key: u32) -> SourcesKey {
    use crate::state::{KEY_BACKSPACE, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_TAB, KEY_UP};
    match key {
        KEY_UP => {
            state.move_selection(-1);
            SourcesKey::None
        }
        KEY_DOWN => {
            state.move_selection(1);
            SourcesKey::None
        }
        KEY_TAB if state.phase == SourcesPhase::Form => {
            state.cycle_trust();
            SourcesKey::None
        }
        KEY_BACKSPACE if state.phase == SourcesPhase::Form => {
            state.pop_field_char();
            SourcesKey::None
        }
        KEY_ENTER => {
            if state.phase == SourcesPhase::Review {
                SourcesKey::ConfirmAdd
            } else {
                SourcesKey::BeginReview
            }
        }
        KEY_ESC => SourcesKey::Back,
        _ => SourcesKey::None,
    }
}

/// Load the repository list from package-service. Transport failures flip the
/// panel to an honest "unavailable" state instead of panicking.
pub(crate) fn refresh_sources(package_handle: rt::Handle, state: &mut AppState) {
    match try_load_repositories(package_handle, &mut state.sources) {
        Ok(count) => {
            state.sources.available = true;
            set_statusf(state, format_args!("sources loaded: {}", count));
        }
        Err(error) => {
            state.sources.available = false;
            state.sources.repo_count = 0;
            set_statusf(
                state,
                format_args!("sources unavailable: {}", error_label(error)),
            );
        }
    }
}

fn try_load_repositories(
    package_handle: rt::Handle,
    sources: &mut SourcesState,
) -> rt::Result<usize> {
    sources.repo_count = 0;
    sources.selected = 0;
    sources.scroll = 0;
    let mut name = [0u8; MAX_REPO_TEXT];
    let mut url = [0u8; MAX_REPO_TEXT];
    for index in 0..MAX_REPOS {
        let Some(info) = rt::package_repository_list(package_handle, index, &mut name, &mut url)?
        else {
            break;
        };
        if info.name_len > MAX_REPO_TEXT || info.url_len > MAX_REPO_TEXT {
            return Err(rt::Error::BufferTooSmall);
        }
        let mut entry = RepoEntry::empty();
        entry.info = info;
        entry.name[..info.name_len].copy_from_slice(&name[..info.name_len]);
        entry.url[..info.url_len].copy_from_slice(&url[..info.url_len]);
        sources.repos[sources.repo_count] = entry;
        sources.repo_count += 1;
    }
    Ok(sources.repo_count)
}

/// Two-phase step 2: send the confirmed add to package-service, then refresh.
pub(crate) fn execute_add(package_handle: rt::Handle, state: &mut AppState) {
    if !state.sources.in_review() {
        return;
    }
    let (name_bytes, name_len) = state.sources.copy_name();
    let (url_bytes, url_len) = state.sources.copy_url();
    let name = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("");
    let url = core::str::from_utf8(&url_bytes[..url_len]).unwrap_or("");
    let trust = state.sources.trust;
    let digest = if trust == PackageRepositoryTrustMode::PinnedDigest {
        state.sources.parse_digest().unwrap_or(0)
    } else {
        0
    };
    match rt::package_repository_add(
        package_handle,
        name,
        url,
        trust,
        PackageChannel::Stable,
        PackageRing::Production,
        true,
        digest,
    ) {
        Ok(()) => {
            state.sources.reset_form();
            state.sources.cancel_review();
            refresh_sources(package_handle, state);
            set_statusf(state, format_args!("repository {} added", name));
        }
        Err(error) => {
            state.sources.cancel_review();
            set_statusf(
                state,
                format_args!("repo add failed: {}", error_label(error)),
            );
        }
    }
}

pub(crate) fn sync_selected(package_handle: rt::Handle, state: &mut AppState) {
    let Some(entry) = state.sources.selected_repo().copied() else {
        return;
    };
    match rt::package_repository_sync(package_handle, Some(entry.info.repo_index as usize)) {
        Ok(sync) => {
            let name = entry.name_text();
            let synced = sync.synced;
            let failed = sync.failed;
            refresh_sources(package_handle, state);
            set_statusf(
                state,
                format_args!("sync {}: {} ok, {} failed", name, synced, failed),
            );
        }
        Err(error) => {
            set_statusf(state, format_args!("sync failed: {}", error_label(error)));
        }
    }
}

/// Honest limitation note: the shell's onboarding ledger (enable/disable/
/// remove, sideload policy) lives in shell-service RAM and is not reachable
/// over IPC, so the GUI routes operators to the shell for those flows.
pub(crate) const LEDGER_NOTE: &str =
    "ledger ops via shell: pkg repo <enable|disable|remove|status>";
pub(crate) const SIDELOAD_NOTE: &str = "sideload policy: shell session (pkg sideload policy)";

pub(crate) fn source_row_meta(entry: &RepoEntry) -> heapless_line::Line {
    let mut line = heapless_line::Line::new();
    let _ = write!(
        &mut line,
        "trust={} sync={} pkgs={} {}",
        trust_mode_name(entry.info.trust_mode),
        sync_state_name(entry.info.sync_state),
        entry.info.package_count,
        if entry.info.enabled {
            "enabled"
        } else {
            "service-disabled"
        },
    );
    line
}

pub(crate) mod heapless_line {
    use core::fmt;

    pub(crate) struct Line {
        bytes: [u8; 96],
        len: usize,
    }

    impl Line {
        pub(crate) const fn new() -> Self {
            Self {
                bytes: [0; 96],
                len: 0,
            }
        }

        pub(crate) fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }

    impl fmt::Write for Line {
        fn write_str(&mut self, piece: &str) -> fmt::Result {
            let bytes = piece.as_bytes();
            let remaining = self.bytes.len() - self.len;
            let take = bytes.len().min(remaining);
            self.bytes[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::compute_layout_for_height;

    fn form() -> SourcesState {
        let mut state = SourcesState::new();
        state.field = AddField::Name;
        for byte in b"third-party" {
            state.push_field_char(*byte);
        }
        state.field = AddField::Url;
        for byte in b"http://feeds.example.com/os/pkg" {
            state.push_field_char(*byte);
        }
        state
    }

    #[test]
    fn trust_meaning_strings_match_shell_review_text() {
        assert_eq!(
            trust_meaning(PackageRepositoryTrustMode::Boot),
            "packages verify against the boot trust root"
        );
        assert_eq!(
            trust_meaning(PackageRepositoryTrustMode::Unsigned),
            "no signature evidence; package bytes are trusted as-fetched"
        );
        assert_eq!(
            trust_meaning(PackageRepositoryTrustMode::PinnedDigest),
            "feed digest must equal your pinned digest on every sync"
        );
        assert_eq!(
            trust_meaning(PackageRepositoryTrustMode::SignedKey),
            "feed must verify against the source key bound at repo add time"
        );
        assert_eq!(
            trust_onboarding_impact(PackageRepositoryTrustMode::Unsigned),
            "every install from this source needs --yes and is flagged unverified"
        );
        assert_eq!(
            trust_onboarding_impact(PackageRepositoryTrustMode::SignedKey),
            "sync fails closed unless the feed verifies under the bound active ed25519 key"
        );
    }

    #[test]
    fn label_strings_match_shell_names() {
        assert_eq!(
            trust_mode_name(PackageRepositoryTrustMode::PinnedDigest),
            "pinned"
        );
        assert_eq!(
            sync_state_name(PackageRepositorySyncState::Offline),
            "offline"
        );
        assert_eq!(repo_channel_name(PackageChannel::Canary), "canary");
        assert_eq!(repo_ring_name(PackageRing::Preview), "preview");
    }

    #[test]
    fn field_input_respects_charset_and_bounds() {
        let mut state = SourcesState::new();
        state.field = AddField::Name;
        assert!(state.push_field_char(b'a'));
        assert!(state.push_field_char(b'-'));
        assert!(state.push_field_char(b'9'));
        assert!(!state.push_field_char(b':')); // URL char rejected in name
        state.field = AddField::Url;
        assert!(state.push_field_char(b':'));
        assert!(state.push_field_char(b'/'));
        state.field = AddField::Digest;
        assert!(state.push_field_char(b'd'));
        assert!(state.push_field_char(b'e'));
        assert!(!state.push_field_char(b'z')); // non-hex rejected
        assert_eq!(state.field_text(AddField::Name), "a-9");
        assert_eq!(state.field_text(AddField::Url), ":/");
        assert_eq!(state.field_text(AddField::Digest), "de");

        let mut long = SourcesState::new();
        long.field = AddField::Name;
        for _ in 0..MAX_NAME_BYTES + 4 {
            long.push_field_char(b'n');
        }
        assert_eq!(long.name_len, MAX_NAME_BYTES);
    }

    #[test]
    fn backspace_pops_focused_field_only() {
        let mut state = form();
        state.field = AddField::Url;
        assert!(state.pop_field_char());
        assert_eq!(state.url_len, 30);
        assert_eq!(state.name_len, 11);
    }

    #[test]
    fn pinned_digest_parsing_accepts_hex_only() {
        let mut state = form();
        state.trust = PackageRepositoryTrustMode::PinnedDigest;
        assert!(state.parse_digest().is_none());
        state.field = AddField::Digest;
        for byte in b"deadbeef" {
            state.push_field_char(*byte);
        }
        assert_eq!(state.parse_digest(), Some(0xdeadbeef));
        state.field = AddField::Digest;
        state.digest_len = 0;
        state.digest = [0; MAX_DIGEST_BYTES];
        for byte in b"0xabc" {
            state.push_field_char(*byte);
        }
        assert_eq!(state.parse_digest(), Some(0xabc));
    }

    #[test]
    fn form_validity_gates_review() {
        let mut state = SourcesState::new();
        assert!(!state.form_valid());
        assert!(!state.begin_review());
        assert_eq!(state.phase, SourcesPhase::Form);

        let mut state = form();
        assert!(state.form_valid());
        assert!(state.begin_review());
        assert_eq!(state.phase, SourcesPhase::Review);
        state.cancel_review();
        assert_eq!(state.phase, SourcesPhase::Form);

        // Pinned trust without a digest is invalid.
        let mut pinned = form();
        pinned.trust = PackageRepositoryTrustMode::PinnedDigest;
        assert!(!pinned.form_valid());
        assert!(!pinned.begin_review());
    }

    #[test]
    fn trust_cycle_clears_digest_when_leaving_pinned() {
        let mut state = form();
        state.trust = PackageRepositoryTrustMode::PinnedDigest;
        state.field = AddField::Digest;
        for byte in b"12" {
            state.push_field_char(*byte);
        }
        state.cycle_trust(); // pinned -> signed-key
        assert_eq!(state.trust, PackageRepositoryTrustMode::SignedKey);
        assert_eq!(state.digest_len, 0);
        state.cycle_trust(); // signed-key -> boot
        state.cycle_trust(); // boot -> unsigned
        state.cycle_trust(); // unsigned -> pinned
        assert_eq!(state.trust, PackageRepositoryTrustMode::PinnedDigest);
    }

    #[test]
    fn add_request_packing_matches_runtime_contract() {
        // Mirrors runtime package_repository_add: word0 = trust | channel<<16
        // | ring<<32 | enabled<<48, word1 = pinned digest.
        let (word0, word1) = plan_add_words(PackageRepositoryTrustMode::PinnedDigest, 0x1234);
        assert_eq!(
            word0 & 0xffff,
            PackageRepositoryTrustMode::PinnedDigest as u64
        );
        assert_eq!((word0 >> 16) & 0xffff, PackageChannel::Stable as u64);
        assert_eq!((word0 >> 32) & 0xffff, PackageRing::Production as u64);
        assert_eq!((word0 >> 48) & 1, 1);
        assert_eq!(word1, 0x1234);
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut state = SourcesState::new();
        state.repo_count = 2;
        state.move_selection(1);
        assert_eq!(state.selected, 1);
        state.move_selection(5);
        assert_eq!(state.selected, 1);
        state.move_selection(-9);
        assert_eq!(state.selected, 0);
        state.selected = 1;
        state.ensure_visible(1);
        assert_eq!(state.scroll, 1);
    }

    #[test]
    fn key_routing_two_phase_flow() {
        let mut state = form();
        assert!(matches!(
            handle_key(&mut state, crate::state::KEY_ENTER),
            SourcesKey::BeginReview
        ));
        // The control layer performs the phase transition the key requests.
        assert!(state.begin_review());
        assert!(state.in_review());
        assert!(matches!(
            handle_key(&mut state, crate::state::KEY_ENTER),
            SourcesKey::ConfirmAdd
        ));
        // Execution (IPC) stays with control; review phase persists until it.
        assert!(state.in_review());
        assert!(matches!(
            handle_key(&mut state, crate::state::KEY_ESC),
            SourcesKey::Back
        ));
        state.cancel_review();
        assert_eq!(state.phase, SourcesPhase::Form);
    }

    #[test]
    fn pointer_routing_targets_form_and_rows() {
        let mut state = SourcesState::new();
        state.repo_count = 1;
        let layout = compute_layout_for_height(768);
        let row_y = layout.list_rows_y + 4;
        assert!(matches!(
            handle_pointer(&state, layout, layout.left_x + 20, row_y),
            SourcesClick::SelectRepo(0)
        ));

        let area = rects(layout, false);
        assert!(matches!(
            handle_pointer(&state, layout, area.field_x0 + 4, area.url_y + 4),
            SourcesClick::Field(AddField::Url)
        ));
        assert!(matches!(
            handle_pointer(&state, layout, area.field_x0 + 4, area.trust_y + 4),
            SourcesClick::CycleTrust
        ));
        assert!(matches!(
            handle_pointer(&state, layout, area.primary_x0 + 4, area.button_y0 + 4),
            SourcesClick::BeginReview
        ));
        assert!(matches!(
            handle_pointer(&state, layout, area.secondary_x0 + 4, area.button_y0 + 4),
            SourcesClick::SyncThis
        ));

        // Review phase reroutes the same button row to confirm/cancel.
        state.phase = SourcesPhase::Review;
        assert!(matches!(
            handle_pointer(&state, layout, area.primary_x0 + 4, area.button_y0 + 4),
            SourcesClick::ConfirmAdd
        ));
        assert!(matches!(
            handle_pointer(&state, layout, area.secondary_x0 + 4, area.button_y0 + 4),
            SourcesClick::CancelReview
        ));
    }

    #[test]
    fn rect_stack_stays_inside_panel_and_ordered() {
        let layout = compute_layout_for_height(768);
        for pinned in [false, true] {
            let area = rects(layout, pinned);
            assert!(area.name_y < area.url_y);
            assert!(area.url_y < area.trust_y);
            assert!(area.trust_y < area.digest_y);
            assert!(area.button_y1 < layout.status_y);
            assert!(area.field_x1 <= layout.right_x + layout.right_w);
            assert!(area.review_y1 < area.button_y0);
        }
    }

    #[test]
    fn row_meta_renders_service_state() {
        let mut entry = RepoEntry::empty();
        entry.info.trust_mode = PackageRepositoryTrustMode::SignedKey;
        entry.info.sync_state = PackageRepositorySyncState::Ready;
        entry.info.package_count = 7;
        entry.info.enabled = false;
        let line = source_row_meta(&entry);
        assert_eq!(
            line.as_str(),
            "trust=signed-key sync=ready pkgs=7 service-disabled"
        );
    }
}
