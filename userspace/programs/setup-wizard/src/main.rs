//! First-boot setup wizard (serial-first onboarding).
//!
//! Lifecycle: the root-manager starts the wizard eagerly once storage and
//! config-service are ready. The wizard immediately registers Ready, then:
//! - marker `state/setup-wizard/firstboot.done` present -> log a skip line
//!   and exit 0 (later boots see no wizard activity beyond one task spawn);
//! - marker absent -> run the serial step machine from
//!   `serviceos_setup_wizard`, apply results (hostname via config-service,
//!   timezone file via storage, admin account provisioned directly into the
//!   persisted account store), write the done-marker, and exit 0.
//!
//! Serial input is polled with a per-step deadline; an operator typing on the
//! serial line drives every step interactively, while headless boots fall
//! through to documented defaults when the deadline expires.
//!
//! Admin account provisioning writes `state/account/accounts.cfg` (shared
//! `serviceos_account_service` store format) through the wizard's own
//! storage access. The account-service picks that file up whenever it is
//! launched, so identity data survives without requiring a mid-setup launch
//! round-trip.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use serviceos_account_service::{ACCOUNTS_PATH, AccountStore, format_store};
use serviceos_setup_wizard::{
    Feed, FieldText, MARKER_PATH, StepId, TIMEZONE_PATH, WizardState, pack_hostname,
};
use serviceos_userspace_runtime as rt;

use rt::{ControlTag, RawMessage};
const MAX_LINE: usize = 96;
const MAX_PATH: usize = 96;
/// Per-step serial input window in ticks before defaults apply. Any received
/// keystroke re-arms the window, so interactive operators keep the step alive
/// while silent (headless) boots fall through to the documented default.
const STEP_TIMEOUT_TICKS: u64 = 400;
const EXIT_OK: u64 = 0;
const EXIT_STARTUP: u64 = 0xfe01;
const EXIT_STORAGE: u64 = 0xfe02;
const EXIT_CONFIG: u64 = 0xfe03;
const EXIT_ACCOUNT: u64 = 0xfe04;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return EXIT_STARTUP;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return EXIT_STARTUP;
    }

    // Become Ready before any decision so bring-up never waits on the
    // interactive part of first boot.
    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return EXIT_STARTUP,
    };
    if rt::register_service(bootstrap, rt::ServiceId::SetupWizard, public.second).is_err() {
        return EXIT_STARTUP;
    }
    let _ = rt::handle_close(public.second);

    let storage_handle = match rt::lookup_service(bootstrap, rt::ServiceId::Storage) {
        Ok(handle) => handle,
        Err(_) => return EXIT_STORAGE,
    };

    if rt::storage_open(storage_handle, MARKER_PATH).is_ok() {
        // Already configured: skip silently.
        let _ = rt::write_logf(
            "setup",
            format_args!("first-boot already complete; skipping"),
        );
        return EXIT_OK;
    }

    let _ = rt::write_logf("setup", format_args!("first boot detected; starting setup"));
    let mut state = WizardState::new();
    while state.step() != StepId::Done {
        serial_print(state.step().prompt());
        match read_line_with_deadline(STEP_TIMEOUT_TICKS) {
            Some(line) => report_feed(&state.feed(line.as_str())),
            None => {
                serial_println("");
                report_feed(&state.feed(""));
            }
        }
    }

    match apply_hostname(bootstrap, state.hostname()) {
        Ok(()) => {}
        Err(reason) => {
            serial_println(reason);
            return EXIT_CONFIG;
        }
    }

    if let Err(error) =
        write_storage_file(storage_handle, TIMEZONE_PATH, state.timezone().as_bytes())
    {
        serial_println("setup: timezone persist failed");
        let _ = error;
        return EXIT_STORAGE;
    }
    serial_println("setup: timezone stored");

    if let Err(reason) =
        create_admin_account(storage_handle, state.admin_name(), state.admin_secret())
    {
        serial_println(reason);
        return EXIT_ACCOUNT;
    }
    serial_println("setup: admin account created");

    let summary = build_summary(&state);
    if write_storage_file(storage_handle, MARKER_PATH, summary.as_bytes()).is_err() {
        serial_println("setup: marker persist failed");
        return EXIT_STORAGE;
    }

    let _ = rt::write_logf(
        "setup",
        format_args!("setup complete: {}", summary.as_str()),
    );
    EXIT_OK
}

fn report_feed(feed: &Feed) {
    if let Feed::Retry(reason) = feed {
        serial_println(reason);
    }
}

/// Persist the chosen hostname through config-service (`system.hostname`,
/// label packed into the u64 value). Returns an operator-facing reason on
/// failure so the serial log explains a failed setup run.
fn apply_hostname(bootstrap: rt::Handle, hostname: &str) -> Result<(), &'static str> {
    let config_handle = match rt::lookup_service(bootstrap, rt::ServiceId::Config) {
        Ok(handle) => handle,
        Err(_) => return Err("setup: config-service unreachable"),
    };
    let hostname_value = match pack_hostname(hostname) {
        Some(value) => value,
        None => {
            let _ = rt::handle_close(config_handle);
            return Err("setup: hostname rejected by local validation");
        }
    };
    if let Err(error) =
        rt::config_write(config_handle, rt::ConfigKey::SystemHostname, hostname_value)
    {
        let _ = rt::handle_close(config_handle);
        serial_println("setup: hostname write failed");
        let _ = error;
        return Err("setup: config-service refused hostname");
    }
    let _ = rt::handle_close(config_handle);
    Ok(())
}

fn build_summary(state: &WizardState) -> FieldText<128> {
    // Reuse the inline-text helper with room for all three values.
    let mut out = FieldText::<128>::empty();
    let mut scratch = [0u8; 128];
    let text = format_args!(
        "hostname={} timezone={} admin={}",
        state.hostname(),
        state.timezone(),
        state.admin_name(),
    );
    let rendered = render(text, &mut scratch);
    out.set(rendered);
    out
}

fn render<'a>(args: core::fmt::Arguments<'_>, scratch: &'a mut [u8]) -> &'a str {
    let mut sink = SliceSink {
        bytes: scratch,
        len: 0,
    };
    let _ = core::fmt::Write::write_fmt(&mut sink, args);
    let len = sink.len;
    core::str::from_utf8(&sink.bytes[..len]).unwrap_or("")
}

struct SliceSink<'a> {
    bytes: &'a mut [u8],
    len: usize,
}

impl<'a> core::fmt::Write for SliceSink<'a> {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let rest = self.bytes.get_mut(self.len..).ok_or(core::fmt::Error)?;
        if value.len() > rest.len() {
            return Err(core::fmt::Error);
        }
        rest[..value.len()].copy_from_slice(value.as_bytes());
        self.len += value.len();
        Ok(())
    }
}

fn serial_print(text: &str) {
    let _ = rt::debug_console_write(text.as_bytes());
}

fn serial_println(text: &str) {
    let _ = rt::debug_console_write(text.as_bytes());
    let _ = rt::debug_console_write(b"\r\n");
}

/// Poll the debug console for one line until `window_ticks` elapse. Each
/// received byte re-arms the window; returns None on timeout (headless
/// boots take the default path).
fn read_line_with_deadline(window_ticks: u64) -> Option<FieldText<MAX_LINE>> {
    let mut line = FieldText::<MAX_LINE>::empty();
    let mut deadline = rt::monotonic_now().unwrap_or(0) + window_ticks;
    loop {
        match rt::debug_console_read_byte() {
            Ok(byte) => {
                deadline = rt::monotonic_now().unwrap_or(0) + window_ticks;
                if byte == b'\r' || byte == b'\n' {
                    // Enter submits whatever was typed (empty = default).
                    return Some(line);
                }
                if (32..127).contains(&byte) {
                    push_byte(&mut line, byte);
                }
            }
            Err(_) => {
                if rt::monotonic_now().unwrap_or(u64::MAX) >= deadline {
                    return None;
                }
                let _ = rt::yield_current();
            }
        }
    }
}

fn push_byte(line: &mut FieldText<MAX_LINE>, byte: u8) {
    let current = line.as_str().as_bytes();
    let mut grown = [0u8; MAX_LINE];
    grown[..current.len()].copy_from_slice(current);
    if let Some(slot) = grown.get_mut(current.len()) {
        *slot = byte;
        if let Ok(text) = core::str::from_utf8(&grown[..current.len() + 1]) {
            line.set(text);
        }
    }
}

fn create_admin_account(
    storage_handle: rt::Handle,
    name: &str,
    secret: &str,
) -> Result<(), &'static str> {
    // Provision the admin account directly into the persisted account store.
    // The wizard already holds a working storage channel, and the shared
    // `serviceos_account_service` format keeps the file authoritative for the
    // account-service when it is launched later (login time), so no mid-setup
    // service launch round-trip is needed on the first-boot critical path.
    let mut store = AccountStore::new();
    store
        .create_account(name, "Administrator", secret.as_bytes(), true)
        .map_err(|_| "setup: admin account rejected")?;
    let mut buffer = [0u8; 2048];
    let written =
        format_store(&store, &mut buffer).map_err(|_| "setup: admin account serialize failed")?;
    write_storage_file(storage_handle, ACCOUNTS_PATH, &buffer[..written])
        .map_err(|_| "setup: admin account persist failed")?;
    Ok(())
}

fn write_storage_file(storage_handle: rt::Handle, path: &str, bytes: &[u8]) -> rt::Result<()> {
    let mut parent = rt::FixedLogBuffer::<MAX_PATH>::new();
    let split_at = path.rfind('/').ok_or(rt::Error::InvalidArgument)?;
    let _ = core::fmt::write(&mut parent, format_args!("{}", &path[..split_at + 1]));
    ensure_directory(storage_handle, parent.as_str())?;
    let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
    let (file, _) = rt::storage_directory_open_file(directory, &path[split_at + 1..], true, true)?;
    let _ = rt::handle_close(directory);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk_len = (bytes.len() - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        let _ = rt::storage_write(
            file,
            offset,
            bytes.len(),
            &bytes[offset..offset + chunk_len],
        )?;
        offset += chunk_len;
    }
    let _ = rt::storage_blob_close(file);
    Ok(())
}

/// Create `path` (trailing-slash directory) if missing, mirroring the
/// config-service helper: probe first, then create level by level.
fn ensure_directory(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    if path.is_empty() || rt::storage_open_directory(storage_handle, path, true).is_ok() {
        return Ok(());
    }
    let mut walked = rt::FixedLogBuffer::<MAX_PATH>::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        let _ = core::fmt::write(&mut walked, format_args!("{}/", segment));
        if rt::storage_open_directory(storage_handle, walked.as_str(), true).is_ok() {
            continue;
        }
        let mut parent = rt::FixedLogBuffer::<MAX_PATH>::new();
        let trimmed = walked.as_str().trim_end_matches('/');
        match trimmed.rsplit_once('/') {
            Some((head, name)) => {
                let _ = core::fmt::write(&mut parent, format_args!("{}/", head));
                let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
                rt::storage_directory_create(directory, name, rt::StorageEntryKind::Directory)?;
                let _ = rt::handle_close(directory);
            }
            None => {
                let directory = rt::storage_open_directory(storage_handle, "/", true)?;
                rt::storage_directory_create(directory, trimmed, rt::StorageEntryKind::Directory)?;
                let _ = rt::handle_close(directory);
            }
        }
    }
    Ok(())
}
