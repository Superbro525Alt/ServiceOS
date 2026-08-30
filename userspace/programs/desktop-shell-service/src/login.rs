//! Graphical login overlay: username + secret entry on the desktop layer
//! driving the account-service LOGIN contract through the shell's own
//! client-session path.
//!
//! Route rationale (no other legal path exists): the desktop shell holds no
//! grant to account-service and the root-manager denies it stored-image
//! launches (`launch_image_is_authorized`), so the overlay opens a shell
//! operator session (`shell_tag::SESSION_OPEN_REQUEST`, the published
//! client-session protocol with zero prior consumers) and submits the same
//! `login <name> <secret>` line the serial console runs. The session stays
//! open after a successful login so the account Owner binding persists for
//! its lifetime.

use serviceos_userspace_runtime as rt;

use crate::{
    KEY_BACKSPACE, KEY_ENTER, KEY_TAB, LOGIN_MESSAGE_MAX, LOGIN_NAME_MAX, LOGIN_SECRET_MAX,
    OverlayMode,
};

/// Wire tags for shell operator sessions, published by
/// `serviceos_shell_service::shell_tag` (kept local to avoid a new crate
/// dependency; values are stable in the shell's reserved range).
mod shell_tag {
    pub const SESSION_OPEN_REQUEST: u32 = 0x240;
    pub const SESSION_OPEN_REPLY: u32 = 0x241;
    pub const SESSION_INPUT_LINE: u32 = 0x242;
    pub const SESSION_OUTPUT_TEXT: u32 = 0x243;
    /// Client is done; releases the operator session row. The desktop keeps
    /// its session open for the binding's lifetime, so only tests reference
    /// the tag — kept here so the protocol stays fully mirrored.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const SESSION_CLOSE: u32 = 0x244;
}

/// Matches `serviceos_shell_service::MAX_LINE_BYTES` (the shell rejects or
/// truncates longer lines).
const MAX_LOGIN_LINE: usize = 128;
const SHELL_PROMPT: &str = "serviceos> ";
/// Bounded drain budget, same shape as the stored-launch announce wait; the
/// shell may itself wait up to ~5000 ticks for account-service to announce.
const REPLY_WAIT_ITERATIONS: usize = 12000;
const OUTPUT_ACC_MAX: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoginPhase {
    Shown,
    Authenticating,
    Failed,
}

pub(crate) struct LoginState {
    pub(crate) phase: LoginPhase,
    pub(crate) name: [u8; LOGIN_NAME_MAX],
    pub(crate) name_len: usize,
    pub(crate) secret: [u8; LOGIN_SECRET_MAX],
    pub(crate) secret_len: usize,
    pub(crate) secret_field_active: bool,
    pub(crate) message: [u8; LOGIN_MESSAGE_MAX],
    pub(crate) message_len: usize,
}

impl LoginState {
    pub(crate) const fn new() -> Self {
        Self {
            phase: LoginPhase::Shown,
            name: [0; LOGIN_NAME_MAX],
            name_len: 0,
            secret: [0; LOGIN_SECRET_MAX],
            secret_len: 0,
            secret_field_active: false,
            message: [0; LOGIN_MESSAGE_MAX],
            message_len: 0,
        }
    }

    fn clear_secret(&mut self) {
        self.secret = [0; LOGIN_SECRET_MAX];
        self.secret_len = 0;
    }

    fn set_message(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let len = bytes.len().min(LOGIN_MESSAGE_MAX);
        self.message[..len].copy_from_slice(&bytes[..len]);
        self.message_len = len;
    }

    pub(crate) fn message_str(&self) -> &str {
        core::str::from_utf8(&self.message[..self.message_len]).unwrap_or("")
    }
}

/// Opens the overlay (palette / Alt+L). Re-opening while shown is a no-op so
/// a stray hotkey cannot wipe in-progress input.
pub(crate) fn open_login_overlay(state: &mut crate::DesktopState) -> rt::Result<u32> {
    if state.overlay_mode != OverlayMode::Login {
        state.overlay_mode = OverlayMode::Login;
        state.login.phase = LoginPhase::Shown;
        state.login.clear_secret();
        state.login.message_len = 0;
        state.login.secret_field_active = false;
    }
    Ok(crate::windows::focused_surface_id(state))
}

/// Overlay dismissed (Esc): wipe the secret immediately, keep nothing.
pub(crate) fn reset_login(state: &mut crate::DesktopState) {
    state.login.clear_secret();
    state.login.phase = LoginPhase::Shown;
    state.login.message_len = 0;
    state.login.secret_field_active = false;
}

/// Result of parsing one shell output burst.
pub(crate) enum LoginOutcome<'a> {
    /// Carries the shell's own "session bound to account=..." line.
    Bound(&'a str),
    /// Carries an operator-readable failure line.
    Failed(&'a str),
}

impl core::fmt::Debug for LoginOutcome<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoginOutcome::Bound(line) => write!(formatter, "Bound({line})"),
            LoginOutcome::Failed(line) => write!(formatter, "Failed({line})"),
        }
    }
}

const BOUND_PREFIX: &str = "session bound to account=";
const FAILURE_MARKERS: [&str; 4] = [
    "login failed",
    "login rejected",
    "account-service",
    "command failed",
];

/// Maps accumulated shell output to a login outcome using the shell's own
/// operator-readable lines (no re-worded drift). Failure lines are surfaced
/// verbatim; anything unrecognizable becomes an honest generic failure.
pub(crate) fn classify_shell_output(text: &str) -> Option<LoginOutcome<'_>> {
    let mut bound = None;
    let mut failed = None;
    for line in text.lines() {
        let line = line.trim();
        if bound.is_none() && line.starts_with(BOUND_PREFIX) {
            bound = Some(line);
        }
        if failed.is_none() && FAILURE_MARKERS.iter().any(|marker| line.contains(marker)) {
            failed = Some(line);
        }
    }
    if let Some(line) = bound {
        return Some(LoginOutcome::Bound(line));
    }
    failed.map(LoginOutcome::Failed)
}

/// Builds the `login <name> <secret>` line within the shell's line budget.
/// Caps mirror the account-service wire limits, matching the shell's own
/// pre-wire validation in `commands::account::login`.
pub(crate) fn build_login_line(
    name: &str,
    secret: &str,
    out: &mut [u8; MAX_LOGIN_LINE],
) -> Option<usize> {
    if name.len() > LOGIN_NAME_MAX || secret.len() > LOGIN_SECRET_MAX {
        return None;
    }
    let prefix = b"login ";
    let name_bytes = name.as_bytes();
    let secret_bytes = secret.as_bytes();
    let total = prefix.len() + name_bytes.len() + 1 + secret_bytes.len();
    if total > out.len() {
        return None;
    }
    let mut len = 0usize;
    out[..prefix.len()].copy_from_slice(prefix);
    len += prefix.len();
    out[len..len + name_bytes.len()].copy_from_slice(name_bytes);
    len += name_bytes.len();
    out[len] = b' ';
    len += 1;
    out[len..len + secret_bytes.len()].copy_from_slice(secret_bytes);
    len += secret_bytes.len();
    Some(len)
}

/// Maps a keyboard scancode to a login-field character (lowercase letters,
/// digits, dot, dash, equals, space); shift uppercases letters. Same
/// vocabulary as the files-app prompts.
pub(crate) fn scancode_to_char(scancode: u32, modifiers: u32) -> Option<u8> {
    const SHIFT: u32 = crate::MOD_SHIFT;
    const ROWS: [(&str, u32); 3] = [("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)];
    let shifted = modifiers & SHIFT != 0;
    let character = match scancode {
        2..=10 => Some(b'1' + (scancode - 2) as u8),
        11 => Some(b'0'),
        12 => Some(if shifted { b'_' } else { b'-' }),
        13 => Some(b'='),
        52 => Some(b'.'),
        57 => Some(b' '),
        _ => ROWS.iter().find_map(|(letters, first)| {
            let offset = scancode.checked_sub(*first)? as usize;
            letters.as_bytes().get(offset).copied()
        }),
    };
    character
        .map(|byte| match (byte.is_ascii_lowercase(), shifted) {
            (true, true) => byte - b'a' + b'A',
            _ => byte,
        })
        .filter(|byte| byte.is_ascii_graphic() || *byte == b' ')
}

/// Pure state-machine step for one key while the overlay is shown. The
/// desktop wrapper turns `Submit` into the shell round-trip; everything
/// here is host-testable without a DesktopState.
pub(crate) enum LoginStep {
    None,
    Submit,
}

impl PartialEq for LoginStep {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (LoginStep::None, LoginStep::None) | (LoginStep::Submit, LoginStep::Submit)
        )
    }
}

impl Eq for LoginStep {}

impl core::fmt::Debug for LoginStep {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoginStep::None => formatter.write_str("None"),
            LoginStep::Submit => formatter.write_str("Submit"),
        }
    }
}

pub(crate) fn login_key_step(login: &mut LoginState, key_code: u32, modifiers: u32) -> LoginStep {
    if login.phase == LoginPhase::Authenticating {
        return LoginStep::None;
    }
    match key_code {
        KEY_TAB => login.secret_field_active = !login.secret_field_active,
        KEY_BACKSPACE => pop_char_state(login),
        KEY_ENTER => {
            let name_len = trimmed_name_len(login);
            if name_len == 0 {
                login.phase = LoginPhase::Failed;
                login.set_message("enter a username");
                return LoginStep::None;
            }
            return LoginStep::Submit;
        }
        _ => {
            if let Some(byte) = scancode_to_char(key_code, modifiers) {
                push_char_state(login, byte);
                if login.phase == LoginPhase::Failed {
                    login.phase = LoginPhase::Shown;
                    login.message_len = 0;
                }
            }
        }
    }
    LoginStep::None
}

fn trimmed_name_len(login: &LoginState) -> usize {
    let bytes = &login.name[..login.name_len];
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |position| position + 1);
    end.saturating_sub(start)
}

fn push_char_state(login: &mut LoginState, byte: u8) {
    if login.secret_field_active {
        if login.secret_len < LOGIN_SECRET_MAX {
            login.secret[login.secret_len] = byte;
            login.secret_len += 1;
        }
    } else if login.name_len < LOGIN_NAME_MAX {
        login.name[login.name_len] = byte;
        login.name_len += 1;
    }
}

fn pop_char_state(login: &mut LoginState) {
    if login.secret_field_active {
        login.secret_len = login.secret_len.saturating_sub(1);
        login.secret[login.secret_len] = 0;
    } else {
        login.name_len = login.name_len.saturating_sub(1);
        login.name[login.name_len] = 0;
    }
}

/// Keyboard handling while the login overlay is shown. Authenticating is
/// atomic (one bounded shell round inside this call) so keys are ignored
/// until it resolves.
pub(crate) fn handle_login_key(
    state: &mut crate::DesktopState,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<u32> {
    match login_key_step(&mut state.login, key_code, modifiers) {
        LoginStep::None => {}
        LoginStep::Submit => submit_login(state)?,
    }
    Ok(crate::windows::focused_surface_id(state))
}

fn submit_login(state: &mut crate::DesktopState) -> rt::Result<()> {
    let mut name_buf = [0u8; LOGIN_NAME_MAX];
    let name_len = trim_copy(&state.login.name[..state.login.name_len], &mut name_buf);
    if name_len == 0 {
        state.login.phase = LoginPhase::Failed;
        state.login.set_message("enter a username");
        return Ok(());
    }

    state.login.phase = LoginPhase::Authenticating;
    let outcome = perform_login(state, &name_buf[..name_len]);
    state.login.clear_secret();
    match outcome {
        Ok(notice) => {
            let _ = crate::windows::post_notification(state, None, false, false, &notice);
            state.overlay_mode = OverlayMode::None;
            state.login.phase = LoginPhase::Shown;
            state.login.message_len = 0;
        }
        Err(failure) => {
            state.login.phase = LoginPhase::Failed;
            state.login.set_message(failure.message());
        }
    }
    Ok(())
}

/// Copies `src` into `out` with ASCII whitespace trimmed; returns the copy
/// length (truncated to `out.len()`).
fn trim_copy(src: &[u8], out: &mut [u8]) -> usize {
    let start = src
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(src.len());
    let end = src
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |position| position + 1);
    if end <= start {
        return 0;
    }
    let slice = &src[start..end];
    let len = slice.len().min(out.len());
    out[..len].copy_from_slice(&slice[..len]);
    len
}

/// Outcome of one login round: `Ok` carries the binding-notice bytes for
/// the desktop notification, `Err` the operator-readable failure message.
type LoginResult = Result<[u8; crate::MAX_NOTIFICATION_BYTES], LoginFailure>;

const TRANSPORT_FAILED: &str = "account-service transport failure";

/// Drives the LOGIN contract through the shell client-session path and
/// returns the resulting notice/failure. Best effort by design: when the
/// shell or account-service is unreachable the overlay says so and the
/// desktop keeps working (account activation is manual; unowned is normal).
fn perform_login(state: &mut crate::DesktopState, name: &[u8]) -> LoginResult {
    let Some(endpoint) = ensure_login_endpoint(state) else {
        return Err(LoginFailure::copy_of(
            "account-service unavailable (shell unreachable or denied)",
        ));
    };

    let mut line = [0u8; MAX_LOGIN_LINE];
    let name_str = core::str::from_utf8(name).unwrap_or("");
    let secret_str =
        core::str::from_utf8(&state.login.secret[..state.login.secret_len]).unwrap_or("");
    let Some(line_len) = build_login_line(name_str, secret_str, &mut line) else {
        return Err(LoginFailure::copy_of(
            "login rejected: credentials too long",
        ));
    };
    let mut request = rt::RawMessage::empty(shell_tag::SESSION_INPUT_LINE);
    let packed = match rt::pack_bytes(&line[..line_len], &mut request.words[1..]) {
        Ok(packed) => packed as usize,
        Err(_) => return Err(LoginFailure::copy_of(TRANSPORT_FAILED)),
    };
    // The packed copy now lives in the request; wipe the plaintext stack copy
    // before any further return path (black_box keeps the memset from being
    // elided as a dead store).
    line = [0u8; MAX_LOGIN_LINE];
    core::hint::black_box(&line);
    request.words[0] = line_len as u64;
    request.word_count = (1 + packed) as u32;
    if rt::channel_send(endpoint, &request).is_err() {
        drop_login_endpoint(state);
        return Err(LoginFailure::copy_of(TRANSPORT_FAILED));
    }

    let mut acc = [0u8; OUTPUT_ACC_MAX];
    let mut acc_len = 0usize;
    let mut answered = false;
    for _ in 0..REPLY_WAIT_ITERATIONS {
        let mut message = rt::RawMessage::empty(0);
        match rt::channel_receive_nonblocking(endpoint, &mut message) {
            Ok(()) => {
                if message.tag == shell_tag::SESSION_OUTPUT_TEXT && message.word_count >= 1 {
                    let room = OUTPUT_ACC_MAX - acc_len;
                    let len = (message.words[0] as usize).min(room);
                    if rt::unpack_bytes(
                        &message.words[1..message.word_count as usize],
                        len,
                        &mut acc[acc_len..],
                    )
                    .is_ok()
                    {
                        acc_len += len;
                    }
                }
                if ends_with_prompt(&acc[..acc_len]) {
                    answered = true;
                    break;
                }
            }
            Err(rt::Error::QueueEmpty) => {
                if rt::yield_current().is_err() {
                    break;
                }
            }
            Err(_) => {
                drop_login_endpoint(state);
                return Err(LoginFailure::copy_of(TRANSPORT_FAILED));
            }
        }
    }

    if !answered {
        return Err(LoginFailure::copy_of(
            "login attempt timed out; account-service may be unavailable",
        ));
    }
    let text = core::str::from_utf8(&acc[..acc_len]).unwrap_or("");
    match classify_shell_output(text) {
        Some(LoginOutcome::Bound(line)) => {
            let mut notice = [0u8; crate::MAX_NOTIFICATION_BYTES];
            let _notice_len = trim_copy(line.as_bytes(), &mut notice);
            Ok(notice)
        }
        Some(LoginOutcome::Failed(reason)) => Err(LoginFailure::copy_of(reason)),
        None => Err(LoginFailure::copy_of("login failed (no reason from shell)")),
    }
}

/// Owned failure message: the shell's operator-readable line copied out of
/// the drain buffer so it can outlive the round-trip.
pub(crate) struct LoginFailure {
    text: [u8; LOGIN_MESSAGE_MAX],
    len: usize,
}

impl LoginFailure {
    fn copy_of(text: &str) -> Self {
        let mut buffer = [0u8; LOGIN_MESSAGE_MAX];
        let bytes = text.as_bytes();
        let len = bytes.len().min(LOGIN_MESSAGE_MAX);
        buffer[..len].copy_from_slice(&bytes[..len]);
        Self { text: buffer, len }
    }

    pub(crate) fn message(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("login failed")
    }
}

fn ends_with_prompt(acc: &[u8]) -> bool {
    let prompt = SHELL_PROMPT.as_bytes();
    acc.len() >= prompt.len() && &acc[acc.len() - prompt.len()..] == prompt
}

/// Cached shell public handle + operator-session endpoint. Both live for the
/// desktop's lifetime; transport errors invalidate them so a later attempt
/// can reopen cleanly (survives a shell restart).
fn shell_public(state: &mut crate::DesktopState) -> Option<rt::Handle> {
    if state.shell_client != rt::INVALID_HANDLE {
        return Some(state.shell_client);
    }
    let handle = rt::lookup_service(state.bootstrap, rt::ServiceId::Shell).ok()?;
    state.shell_client = handle;
    Some(handle)
}

fn drop_login_endpoint(state: &mut crate::DesktopState) {
    if state.login_endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(state.login_endpoint);
        state.login_endpoint = rt::INVALID_HANDLE;
    }
}

fn ensure_login_endpoint(state: &mut crate::DesktopState) -> Option<rt::Handle> {
    if state.login_endpoint != rt::INVALID_HANDLE {
        return Some(state.login_endpoint);
    }
    let shell = shell_public(state)?;
    let endpoint = open_shell_session(shell)?;
    state.login_endpoint = endpoint;
    Some(endpoint)
}

/// SESSION_OPEN_REQUEST dance: reply channel in handles[0], the reply's
/// handles[0] is this session's endpoint (SEND|RECEIVE|DUPLICATE). Bounded
/// wait, same shape as the stored-launch announce wait.
fn open_shell_session(shell: rt::Handle) -> Option<rt::Handle> {
    const OPEN_WAIT_ITERATIONS: usize = 4000;

    let pair = rt::channel_create().ok()?;
    let mut request = rt::RawMessage::empty(shell_tag::SESSION_OPEN_REQUEST);
    request.handle_count = 1;
    request.handles[0] = pair.second;
    request.handle_rights[0] = rt::rights::SEND;
    if rt::channel_send(shell, &request).is_err() {
        let _ = rt::handle_close(pair.first);
        let _ = rt::handle_close(pair.second);
        return None;
    }
    let _ = rt::handle_close(pair.second);

    for _ in 0..OPEN_WAIT_ITERATIONS {
        let mut reply = rt::RawMessage::empty(0);
        match rt::channel_receive_nonblocking(pair.first, &mut reply) {
            Ok(()) => {
                let _ = rt::handle_close(pair.first);
                if reply.tag == shell_tag::SESSION_OPEN_REPLY
                    && reply.word_count >= 1
                    && reply.words[0] == 0
                    && reply.handle_count >= 1
                {
                    return Some(reply.handles[0]);
                }
                return None;
            }
            Err(rt::Error::QueueEmpty) => {
                if rt::yield_current().is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = rt::handle_close(pair.first);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KEY_ENTER, KEY_L, MOD_ALT, MOD_SHIFT};

    fn key_of(letter: u8) -> u32 {
        // QWERTY scancodes for a..z (linux/input-event-codes.h vocabulary).
        const ROWS: [(&str, u32); 3] = [("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)];
        ROWS.iter()
            .find_map(|(letters, first)| {
                letters
                    .as_bytes()
                    .iter()
                    .position(|candidate| *candidate == letter)
                    .map(|offset| first + offset as u32)
            })
            .unwrap_or(0)
    }

    #[test]
    fn scancode_mapping_matches_files_app_vocabulary() {
        assert_eq!(scancode_to_char(key_of(b'b'), 0), Some(b'b'));
        assert_eq!(scancode_to_char(key_of(b'b'), MOD_SHIFT), Some(b'B'));
        assert_eq!(scancode_to_char(2, 0), Some(b'1'));
        assert_eq!(scancode_to_char(11, 0), Some(b'0'));
        assert_eq!(scancode_to_char(12, 0), Some(b'-'));
        assert_eq!(scancode_to_char(12, MOD_SHIFT), Some(b'_'));
        assert_eq!(scancode_to_char(52, 0), Some(b'.'));
        assert_eq!(scancode_to_char(57, 0), Some(b' '));
        assert_eq!(scancode_to_char(1, 0), None);
    }

    #[test]
    fn state_machine_types_toggles_and_erases() {
        let mut login = LoginState::new();
        for letter in b"bob" {
            assert_eq!(
                login_key_step(&mut login, key_of(*letter), 0),
                LoginStep::None
            );
        }
        assert_eq!(&login.name[..login.name_len], b"bob");
        assert_eq!(
            login_key_step(&mut login, crate::KEY_TAB, 0),
            LoginStep::None
        );
        assert!(login.secret_field_active);
        for letter in b"pw" {
            login_key_step(&mut login, key_of(*letter), 0);
        }
        assert_eq!(&login.secret[..login.secret_len], b"pw");
        login_key_step(&mut login, crate::KEY_BACKSPACE, 0);
        assert_eq!(&login.secret[..login.secret_len], b"p");
        login_key_step(&mut login, crate::KEY_TAB, 0);
        login_key_step(&mut login, crate::KEY_BACKSPACE, 0);
        assert_eq!(&login.name[..login.name_len], b"bo");
    }

    #[test]
    fn empty_name_submit_is_refused_without_a_shell_round() {
        let mut login = LoginState::new();
        assert_eq!(
            login_key_step(&mut login, KEY_ENTER, 0),
            LoginStep::None,
            "empty name must not reach the shell"
        );
        assert_eq!(login.phase, LoginPhase::Failed);
        assert_eq!(login.message_str(), "enter a username");
        let mut blank = LoginState::new();
        blank.name_len = 2;
        blank.name[..2].copy_from_slice(b"  ");
        assert_eq!(login_key_step(&mut blank, KEY_ENTER, 0), LoginStep::None);
    }

    #[test]
    fn typing_after_failure_returns_to_editable_shown() {
        let mut login = LoginState::new();
        login.phase = LoginPhase::Failed;
        login.set_message("login rejected: bad credentials");
        login_key_step(&mut login, key_of(b'a'), 0);
        assert_eq!(login.phase, LoginPhase::Shown);
        assert_eq!(login.message_len, 0);
        assert_eq!(&login.name[..login.name_len], b"a");
    }

    #[test]
    fn authenticating_ignores_all_keys() {
        let mut login = LoginState::new();
        login.phase = LoginPhase::Authenticating;
        assert_eq!(login_key_step(&mut login, KEY_ENTER, 0), LoginStep::None);
        assert_eq!(
            login_key_step(&mut login, crate::KEY_TAB, 0),
            LoginStep::None
        );
        assert_eq!(
            login_key_step(&mut login, crate::KEY_BACKSPACE, 0),
            LoginStep::None
        );
        assert_eq!(login_key_step(&mut login, key_of(b'x'), 0), LoginStep::None);
        assert_eq!(login.name_len, 0);
    }

    #[test]
    fn login_line_matches_shell_command_shape_and_budget() {
        let mut line = [0u8; MAX_LOGIN_LINE];
        let len = build_login_line("paul", "hunter2", &mut line).expect("fits");
        assert_eq!(&line[..len], b"login paul hunter2");
        assert!(len <= 128, "must stay within the shell line budget");

        let long_name = "n".repeat(LOGIN_NAME_MAX + 1);
        assert_eq!(build_login_line(&long_name, "x", &mut line), None);
        let long_secret = "s".repeat(LOGIN_SECRET_MAX + 1);
        assert_eq!(build_login_line("ok", &long_secret, &mut line), None);
    }

    #[test]
    fn shell_output_maps_bound_failed_and_unknown() {
        let bound = "session bound to account=paul id=1 capabilities=0x3\r\nserviceos> ";
        match classify_shell_output(bound) {
            Some(LoginOutcome::Bound(line)) => {
                assert!(line.starts_with(BOUND_PREFIX));
            }
            other => panic!("expected bound, got {other:?}"),
        }

        for failed in [
            "login rejected: bad credentials\r\nserviceos> ",
            "login failed: session vanished\r\nserviceos> ",
            "account-service unavailable (not in boot store or launch denied); \
             session stays unowned\r\nserviceos> ",
        ] {
            match classify_shell_output(failed) {
                Some(LoginOutcome::Failed(reason)) => {
                    assert!(FAILURE_MARKERS.iter().any(|marker| reason.contains(marker)));
                }
                other => panic!("expected failure for {failed:?}, got {other:?}"),
            }
        }

        assert!(classify_shell_output("serviceos> ").is_none());
        assert!(classify_shell_output("").is_none());

        let mixed =
            "login rejected: x\r\nsession bound to account=paul id=1 capabilities=0\r\nserviceos> ";
        assert!(matches!(
            classify_shell_output(mixed),
            Some(LoginOutcome::Bound(_))
        ));
    }

    #[test]
    fn overlay_is_registry_reachable_via_alt_l() {
        let entry = crate::actions::action_for_binding(MOD_ALT, KEY_L)
            .expect("Alt+L must open the login overlay");
        assert_eq!(entry, crate::PaletteAction::ShowLogin);
        assert_eq!(
            crate::actions::action_label(crate::PaletteAction::ShowLogin),
            "Login"
        );
    }

    #[test]
    fn route_tags_match_the_published_shell_protocol() {
        // Guard against silent drift from serviceos_shell_service::shell_tag.
        assert_eq!(shell_tag::SESSION_OPEN_REQUEST, 0x240);
        assert_eq!(shell_tag::SESSION_OPEN_REPLY, 0x241);
        assert_eq!(shell_tag::SESSION_INPUT_LINE, 0x242);
        assert_eq!(shell_tag::SESSION_OUTPUT_TEXT, 0x243);
        assert_eq!(shell_tag::SESSION_CLOSE, 0x244);
    }

    #[test]
    fn trim_copy_strips_whitespace_and_caps() {
        let mut out = [0u8; 4];
        assert_eq!(trim_copy(b"  paul  ", &mut out), 4);
        assert_eq!(&out, b"paul");
        assert_eq!(trim_copy(b"   ", &mut out), 0);
        assert_eq!(trim_copy(b"", &mut out), 0);
    }
}
