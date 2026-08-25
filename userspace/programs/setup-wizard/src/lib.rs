//! First-boot setup wizard: pure step-sequencing logic shared between the
//! `no_std` service binary and host unit tests, mirroring the account-service
//! layout.
//!
//! The wizard drives a serial-first onboarding conversation:
//! hostname -> timezone -> admin account name/secret -> confirm.
//! Empty input applies a documented default so headless boots (no operator
//! on the serial line) complete deterministically; invalid input retries the
//! current step. Completion writes the done-marker so later boots skip.
//!
//! Marker and state paths (all under the wizard's own storage tree):
//! - `state/setup-wizard/firstboot.done`  presence == system configured
//! - `state/setup-wizard/timezone.txt`    free-text timezone label
//!
//! Hostname persists through config-service as `system.hostname`
//! (`ConfigKey::SystemHostname`): a hostname label of at most 8 ASCII bytes
//! packed big-endian into the u64 config value.

#![cfg_attr(not(test), no_std)]

pub const MARKER_PATH: &str = "state/setup-wizard/firstboot.done";
pub const TIMEZONE_PATH: &str = "state/setup-wizard/timezone.txt";
pub const WIZARD_DIR: &str = "state/setup-wizard/";

pub const DEFAULT_HOSTNAME: &str = "svc-os";
pub const DEFAULT_TIMEZONE: &str = "UTC";
pub const DEFAULT_ADMIN_NAME: &str = "admin";
pub const DEFAULT_ADMIN_SECRET: &str = "serviceos";

pub const HOSTNAME_MAX: usize = 8;
pub const TIMEZONE_MAX: usize = 24;
pub const ADMIN_NAME_MAX: usize = 32;
pub const ADMIN_SECRET_MAX: usize = 64;

/// A validated short string kept inline for `no_std` use.
#[derive(Clone, Copy)]
pub struct FieldText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FieldText<N> {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn set(&mut self, value: &str) -> bool {
        let bytes = value.as_bytes();
        if bytes.len() > N {
            return false;
        }
        self.bytes = [0; N];
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len();
        true
    }

    pub fn as_str(&self) -> &str {
        // Only set() with validated ASCII reaches here in practice; fall back
        // to the longest valid prefix rather than panicking.
        let mut len = self.len;
        while core::str::from_utf8(&self.bytes[..len]).is_err() {
            len -= 1;
        }
        core::str::from_utf8(&self.bytes[..len]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// A hostname label: 1..=8 ASCII letters, digits, or hyphens, starting with
/// an alphanumeric byte (matches the config-service packing validator).
pub fn is_hostname_label(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > HOSTNAME_MAX || !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Pack a validated hostname label big-endian into a u64 config value.
pub fn pack_hostname(label: &str) -> Option<u64> {
    if !is_hostname_label(label) {
        return None;
    }
    let mut packed = [0u8; 8];
    packed[..label.len()].copy_from_slice(label.as_bytes());
    Some(u64::from_be_bytes(packed))
}

/// Inverse of [`pack_hostname`] for display back the stored value.
pub fn unpack_hostname(value: u64) -> Option<FieldText<HOSTNAME_MAX>> {
    let bytes = value.to_be_bytes();
    let mut len = 0usize;
    while len < 8 && bytes[len] != 0 {
        len += 1;
    }
    if bytes[len..].iter().any(|byte| *byte != 0) {
        return None;
    }
    let text = FieldText::<HOSTNAME_MAX>::empty();
    let Ok(label) = core::str::from_utf8(&bytes[..len]) else {
        return None;
    };
    if !is_hostname_label(label) {
        return None;
    }
    let mut out = text;
    out.set(label);
    Some(out)
}

fn is_free_text(value: &str, max: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty() && bytes.len() <= max && bytes.iter().all(|b| (33..=126).contains(b))
}

fn is_account_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > ADMIN_NAME_MAX {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b'_')
}

fn is_admin_secret(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= ADMIN_SECRET_MAX
        && bytes.iter().all(|b| (32..127).contains(b))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepId {
    Hostname,
    Timezone,
    AdminName,
    AdminSecret,
    Confirm,
    Done,
}

impl StepId {
    pub fn prompt(self) -> &'static str {
        match self {
            StepId::Hostname => "setup: hostname label (1-8 chars, Enter = svc-os): ",
            StepId::Timezone => "setup: timezone label (Enter = UTC): ",
            StepId::AdminName => "setup: admin account name (Enter = admin): ",
            StepId::AdminSecret => "setup: admin account secret (Enter = serviceos): ",
            StepId::Confirm => "setup: apply these values? type y to apply, n to restart: ",
            StepId::Done => "",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Feed {
    /// Input accepted; move to the next step.
    Advance,
    /// Input rejected; stay on this step (`reason` is operator-facing).
    Retry(&'static str),
    /// Confirm accepted; wizard state is final.
    Complete,
    /// Confirm declined; restart from the first step.
    Restart,
}

#[derive(Clone, Copy)]
pub struct WizardState {
    step: StepId,
    hostname: FieldText<HOSTNAME_MAX>,
    timezone: FieldText<TIMEZONE_MAX>,
    admin_name: FieldText<ADMIN_NAME_MAX>,
    admin_secret: FieldText<ADMIN_SECRET_MAX>,
}

impl WizardState {
    pub fn new() -> Self {
        Self {
            step: StepId::Hostname,
            hostname: FieldText::empty(),
            timezone: FieldText::empty(),
            admin_name: FieldText::empty(),
            admin_secret: FieldText::empty(),
        }
    }

    pub fn step(&self) -> StepId {
        self.step
    }

    pub fn hostname(&self) -> &str {
        self.hostname.as_str()
    }

    pub fn timezone(&self) -> &str {
        self.timezone.as_str()
    }

    pub fn admin_name(&self) -> &str {
        self.admin_name.as_str()
    }

    pub fn admin_secret(&self) -> &str {
        self.admin_secret.as_str()
    }

    /// Feed one serial input line into the current step. An empty line means
    /// "accept the default" (headless boots time out into this path).
    pub fn feed(&mut self, line: &str) -> Feed {
        let line = line.trim();
        match self.step {
            StepId::Hostname => {
                let value = if line.is_empty() {
                    DEFAULT_HOSTNAME
                } else {
                    line
                };
                if is_hostname_label(value) && self.hostname.set(value) {
                    self.step = StepId::Timezone;
                    Feed::Advance
                } else {
                    Feed::Retry("hostname must be 1-8 letters/digits/hyphen")
                }
            }
            StepId::Timezone => {
                let value = if line.is_empty() {
                    DEFAULT_TIMEZONE
                } else {
                    line
                };
                if is_free_text(value, TIMEZONE_MAX) && self.timezone.set(value) {
                    self.step = StepId::AdminName;
                    Feed::Advance
                } else {
                    Feed::Retry("timezone must be 1-24 printable characters")
                }
            }
            StepId::AdminName => {
                let value = if line.is_empty() {
                    DEFAULT_ADMIN_NAME
                } else {
                    line
                };
                if is_account_name(value) && self.admin_name.set(value) {
                    self.step = StepId::AdminSecret;
                    Feed::Advance
                } else {
                    Feed::Retry("account name must be 1-32 letters/digits/-/_")
                }
            }
            StepId::AdminSecret => {
                let value = if line.is_empty() {
                    DEFAULT_ADMIN_SECRET
                } else {
                    line
                };
                if is_admin_secret(value) && self.admin_secret.set(value) {
                    self.step = StepId::Confirm;
                    Feed::Advance
                } else {
                    Feed::Retry("secret must be 1-64 non-control characters")
                }
            }
            StepId::Confirm => match line {
                "" | "y" | "yes" => {
                    self.step = StepId::Done;
                    Feed::Complete
                }
                "n" | "no" => {
                    *self = WizardState::new();
                    Feed::Restart
                }
                _ => Feed::Retry("answer y or n"),
            },
            StepId::Done => Feed::Complete,
        }
    }
}

impl Default for WizardState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_flow_reaches_done_with_documented_defaults() {
        let mut state = WizardState::new();
        assert_eq!(state.step(), StepId::Hostname);
        assert_eq!(state.feed(""), Feed::Advance);
        assert_eq!(state.hostname(), DEFAULT_HOSTNAME);
        assert_eq!(state.feed(""), Feed::Advance);
        assert_eq!(state.timezone(), DEFAULT_TIMEZONE);
        assert_eq!(state.feed(""), Feed::Advance);
        assert_eq!(state.admin_name(), DEFAULT_ADMIN_NAME);
        assert_eq!(state.feed(""), Feed::Advance);
        assert_eq!(state.admin_secret(), DEFAULT_ADMIN_SECRET);
        assert_eq!(state.step(), StepId::Confirm);
        assert_eq!(state.feed("y"), Feed::Complete);
        assert_eq!(state.step(), StepId::Done);
        assert_eq!(state.feed("anything"), Feed::Complete);
    }

    #[test]
    fn custom_values_flow_through_all_steps() {
        let mut state = WizardState::new();
        assert_eq!(state.feed("box-1"), Feed::Advance);
        assert_eq!(state.hostname(), "box-1");
        assert_eq!(state.feed("Europe/Berlin"), Feed::Advance);
        assert_eq!(state.timezone(), "Europe/Berlin");
        assert_eq!(state.feed("paul"), Feed::Advance);
        assert_eq!(state.admin_name(), "paul");
        assert_eq!(state.feed("s3cret!"), Feed::Advance);
        assert_eq!(state.feed("y"), Feed::Complete);
    }

    #[test]
    fn invalid_input_retries_same_step() {
        let mut state = WizardState::new();
        assert_eq!(
            state.feed("way-too-long-hostname"),
            Feed::Retry("hostname must be 1-8 letters/digits/hyphen")
        );
        assert_eq!(
            state.feed("-bad"),
            Feed::Retry("hostname must be 1-8 letters/digits/hyphen")
        );
        assert_eq!(state.feed("ok-9"), Feed::Advance);

        assert_eq!(
            state.feed("has space"),
            Feed::Retry("timezone must be 1-24 printable characters")
        );
        assert_eq!(state.feed(""), Feed::Advance);

        assert_eq!(
            state.feed("bad name"),
            Feed::Retry("account name must be 1-32 letters/digits/-/_")
        );
        assert_eq!(state.feed("root_2"), Feed::Advance);

        assert_eq!(
            state.feed("\u{7f}"),
            Feed::Retry("secret must be 1-64 non-control characters")
        );
        assert_eq!(state.feed(""), Feed::Advance);
        assert_eq!(state.feed("maybe"), Feed::Retry("answer y or n"));
        assert_eq!(state.feed("y"), Feed::Complete);
    }

    #[test]
    fn confirm_no_restarts_from_first_step() {
        let mut state = WizardState::new();
        for _ in 0..4 {
            assert_eq!(state.feed(""), Feed::Advance);
        }
        assert_eq!(state.feed("n"), Feed::Restart);
        assert_eq!(state.step(), StepId::Hostname);
        // Restarted wizard keeps no stale values.
        assert_eq!(state.hostname(), "");
        assert_eq!(state.timezone(), "");
        assert_eq!(state.admin_name(), "");
    }

    #[test]
    fn hostname_packing_roundtrips_and_validates() {
        assert_eq!(pack_hostname("svc-os"), Some(0x73_76_63_2d_6f_73_00_00u64));
        let restored = unpack_hostname(0x73_76_63_2d_6f_73_00_00u64).expect("roundtrip");
        assert_eq!(restored.as_str(), "svc-os");
        assert!(pack_hostname("").is_none());
        assert!(pack_hostname("nine-char").is_none());
        assert!(pack_hostname("-lead").is_none());
        assert!(pack_hostname("has space").is_none());
        assert!(unpack_hostname(u64::MAX).is_none());
    }

    #[test]
    fn marker_and_state_paths_stay_under_wizard_tree() {
        assert!(MARKER_PATH.starts_with(WIZARD_DIR));
        assert!(TIMEZONE_PATH.starts_with(WIZARD_DIR));
        assert!(!MARKER_PATH.ends_with("/"));
    }
}
