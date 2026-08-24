//! Account store, login/identity state machine, and per-account policy
//! defaults for the account service. Pure logic plus the service's own wire
//! tags and byte packing helpers, shared between the `no_std` service binary
//! and host unit tests.
//!
//! Credential hashing note: the store uses a salted, stretched FNV-1a based
//! key derivation (`derive_auth_hash`). FNV-1a is NOT a cryptographic hash;
//! this is an honest placeholder KDF for a pragmatic foundation until a real
//! password hashing scheme (e.g. PBKDF2/Argon2) lands. Salts are per-account
//! and stored alongside the derived hash, shadow-file style.

#![cfg_attr(not(test), no_std)]

pub const MAX_ACCOUNTS: usize = 8;
pub const MAX_NAME: usize = 32;
pub const MAX_DISPLAY: usize = 48;
pub const MAX_OWNED_SESSIONS: usize = 4;

/// Store file path, relative to the storage root granted at startup.
pub const ACCOUNTS_PATH: &str = "state/account/accounts.cfg";

/// KDF stretching rounds for `derive_auth_hash`.
pub const KDF_ROUNDS: u32 = 8;

/// Wire tag base chosen away from existing service ranges.
pub mod account_tag {
    pub const CREATE_REQUEST: u32 = 0x211;
    pub const CREATE_REPLY: u32 = 0x212;
    pub const LOGIN_REQUEST: u32 = 0x213;
    pub const LOGIN_REPLY: u32 = 0x214;
    pub const LOGOUT_REQUEST: u32 = 0x215;
    pub const LOGOUT_REPLY: u32 = 0x216;
    pub const SWITCH_REQUEST: u32 = 0x217;
    pub const SWITCH_REPLY: u32 = 0x218;
    pub const POLICY_GET_REQUEST: u32 = 0x219;
    pub const POLICY_GET_REPLY: u32 = 0x21A;
    pub const LIST_REQUEST: u32 = 0x21B;
    pub const LIST_REPLY: u32 = 0x21C;
}

/// Per-account default capability grant bits recorded for future enforcement
/// points (storage, shell, package, runtime, session switching).
pub mod capability {
    pub const STORAGE_READ: u32 = 1 << 0;
    pub const STORAGE_WRITE: u32 = 1 << 1;
    pub const LOG_SEND: u32 = 1 << 2;
    pub const NETWORK_USE: u32 = 1 << 3;
    pub const SESSION_SWITCH: u32 = 1 << 4;
    pub const PACKAGE_INSTALL: u32 = 1 << 5;
    pub const DEVELOPER_TOOLS: u32 = 1 << 6;

    pub const ADMIN_DEFAULT: u32 = STORAGE_READ
        | STORAGE_WRITE
        | LOG_SEND
        | NETWORK_USE
        | SESSION_SWITCH
        | PACKAGE_INSTALL
        | DEVELOPER_TOOLS;

    pub const OPERATOR_DEFAULT: u32 = STORAGE_READ | LOG_SEND | SESSION_SWITCH;
}

/// Default capability grant set for a freshly created account.
pub fn default_capabilities(is_admin: bool) -> u32 {
    if is_admin {
        capability::ADMIN_DEFAULT
    } else {
        capability::OPERATOR_DEFAULT
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountError {
    InvalidArgument,
    CapacityExceeded,
    DuplicateName,
    UnknownAccount,
    BadCredentials,
    NotLoggedIn,
    AlreadyLoggedIn,
    SessionAlreadyClaimed,
    StoreCorrupt,
}

impl AccountError {
    /// Wire status code: 0 = Ok, errors count up from 1.
    pub fn to_code(self) -> u32 {
        match self {
            AccountError::InvalidArgument => 1,
            AccountError::CapacityExceeded => 2,
            AccountError::DuplicateName => 3,
            AccountError::UnknownAccount => 4,
            AccountError::BadCredentials => 5,
            AccountError::NotLoggedIn => 6,
            AccountError::AlreadyLoggedIn => 7,
            AccountError::SessionAlreadyClaimed => 8,
            AccountError::StoreCorrupt => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Account {
    pub id: u32,
    pub name_len: usize,
    pub name: [u8; MAX_NAME],
    pub display_len: usize,
    pub display: [u8; MAX_DISPLAY],
    pub salt: u64,
    pub auth_hash: u64,
    pub capabilities: u32,
    pub owned_sessions: [u32; MAX_OWNED_SESSIONS],
    pub owned_count: usize,
}

/// Active login claim: which account owns which session-service session id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim {
    pub account_id: u32,
    pub session_id: u32,
}

/// Record of one identity switch, mirroring session-service handoff
/// semantics (identity follows the operator from the outgoing session to the
/// incoming session; seat/input routing itself stays delegated to
/// session-service).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SwitchRecord {
    pub account_id: u32,
    pub from_session: u32,
    pub to_session: u32,
}

pub const EMPTY_ACCOUNT: Account = Account {
    id: 0,
    name_len: 0,
    name: [0; MAX_NAME],
    display_len: 0,
    display: [0; MAX_DISPLAY],
    salt: 0,
    auth_hash: 0,
    capabilities: 0,
    owned_sessions: [0; MAX_OWNED_SESSIONS],
    owned_count: 0,
};

/// FNV-1a 64-bit over bytes (same primitive family as package-service).
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Salted, stretched FNV-1a KDF. NON-CRYPTOGRAPHIC placeholder; see module
/// docs. Deterministic for identical (salt, secret) inputs.
pub fn derive_auth_hash(salt: u64, secret: &[u8]) -> u64 {
    let mut hash = fnv1a64(&salt.to_le_bytes());
    for round in 0..KDF_ROUNDS {
        for byte in secret {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash ^= round as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^ fnv1a64(&(salt.wrapping_mul(0x9e37_79b9_7f4a_7c15)).to_le_bytes())
}

/// Account registry plus the single active login claim.
///
/// Login state machine: `LoggedOut` --login(ok)--> `LoggedIn`;
/// logout returns to `LoggedOut`; switch_user re-binds the active claim to a
/// different session id without re-authenticating.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountStore {
    pub accounts: [Account; MAX_ACCOUNTS],
    pub count: usize,
    pub next_id: u32,
    pub active: Option<Claim>,
}

impl AccountStore {
    pub const fn new() -> Self {
        Self {
            accounts: [EMPTY_ACCOUNT; MAX_ACCOUNTS],
            count: 0,
            next_id: 1,
            active: None,
        }
    }

    /// Bootstrap accounts seeded when no persisted store exists. Placeholder
    /// credentials; changing them is expected before real use.
    pub fn seed_defaults() -> Self {
        let mut store = Self::new();
        let _ = store.create_account("admin", "Administrator", b"admin", true);
        let _ = store.create_account("operator", "Operator", b"operator", false);
        store
    }

    pub fn account_index(&self, name: &[u8]) -> Option<usize> {
        (0..self.count).find(|index| {
            let account = &self.accounts[*index];
            account.name_len == name.len() && &account.name[..name.len()] == name
        })
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Account> {
        self.account_index(name.as_bytes())
            .map(|i| &self.accounts[i])
    }

    /// Create an account with a fresh per-account salt and the default grant
    /// set for its class. Returns the new account id.
    pub fn create_account(
        &mut self,
        name: &str,
        display: &str,
        secret: &[u8],
        is_admin: bool,
    ) -> Result<u32, AccountError> {
        if name.is_empty() || name.len() > MAX_NAME || display.len() > MAX_DISPLAY {
            return Err(AccountError::InvalidArgument);
        }
        if secret.is_empty() {
            return Err(AccountError::InvalidArgument);
        }
        if self.count >= MAX_ACCOUNTS {
            return Err(AccountError::CapacityExceeded);
        }
        if self.find_by_name(name).is_some() {
            return Err(AccountError::DuplicateName);
        }
        let slot = self.count;
        let id = self.next_id;
        // Salt mixes the name with a fixed domain constant so distinct
        // accounts never share a salt even when created in one pass.
        let salt = fnv1a64(b"account-service/salt") ^ fnv1a64(name.as_bytes()) ^ id as u64;
        self.accounts[slot] = Account {
            id,
            name_len: name.len(),
            name: copy_field::<MAX_NAME>(name.as_bytes()),
            display_len: display.len(),
            display: copy_field::<MAX_DISPLAY>(display.as_bytes()),
            salt,
            auth_hash: derive_auth_hash(salt, secret),
            capabilities: default_capabilities(is_admin),
            owned_sessions: [0; MAX_OWNED_SESSIONS],
            owned_count: 0,
        };
        self.count += 1;
        self.next_id += 1;
        Ok(id)
    }

    fn verify(&self, index: usize, secret: &[u8]) -> bool {
        let account = &self.accounts[index];
        derive_auth_hash(account.salt, secret) == account.auth_hash
    }

    /// Verify credentials without mutating state.
    pub fn check_credentials(&self, name: &str, secret: &[u8]) -> Result<usize, AccountError> {
        let Some(index) = self.account_index(name.as_bytes()) else {
            return Err(AccountError::UnknownAccount);
        };
        if !self.verify(index, secret) {
            return Err(AccountError::BadCredentials);
        }
        Ok(index)
    }

    /// login(name, secret, session): verify credentials and claim ownership
    /// of a session-service session id.
    pub fn login(
        &mut self,
        name: &str,
        secret: &[u8],
        session_id: u32,
    ) -> Result<Claim, AccountError> {
        if session_id == 0 {
            return Err(AccountError::InvalidArgument);
        }
        if self.active.is_some() {
            return Err(AccountError::AlreadyLoggedIn);
        }
        let index = self.check_credentials(name, secret)?;
        let claim = Claim {
            account_id: self.accounts[index].id,
            session_id,
        };
        self.record_ownership(index, session_id)?;
        self.active = Some(claim);
        Ok(claim)
    }

    /// logout(session): drop the active claim if it still owns `session`.
    pub fn logout(&mut self, session_id: u32) -> Result<Claim, AccountError> {
        let Some(claim) = self.active else {
            return Err(AccountError::NotLoggedIn);
        };
        if claim.session_id != session_id {
            return Err(AccountError::SessionAlreadyClaimed);
        }
        let index = self
            .account_index_of_id(claim.account_id)
            .ok_or(AccountError::UnknownAccount)?;
        self.release_ownership(index, session_id);
        self.active = None;
        Ok(claim)
    }

    /// switch_user(target_session): move the active identity claim onto a
    /// different session id, mirroring session-service's staged handoff from
    /// the caller's perspective. The outgoing binding is released.
    pub fn switch_user(&mut self, target_session: u32) -> Result<SwitchRecord, AccountError> {
        if target_session == 0 {
            return Err(AccountError::InvalidArgument);
        }
        let claim = self.active.ok_or(AccountError::NotLoggedIn)?;
        if claim.session_id == target_session {
            return Err(AccountError::SessionAlreadyClaimed);
        }
        let index = self
            .account_index_of_id(claim.account_id)
            .ok_or(AccountError::UnknownAccount)?;
        self.release_ownership(index, claim.session_id);
        self.record_ownership(index, target_session)?;
        self.active = Some(Claim {
            account_id: claim.account_id,
            session_id: target_session,
        });
        Ok(SwitchRecord {
            account_id: claim.account_id,
            from_session: claim.session_id,
            to_session: target_session,
        })
    }

    pub fn capabilities_of(&self, account_id: u32) -> Result<u32, AccountError> {
        self.account_index_of_id(account_id)
            .map(|index| self.accounts[index].capabilities)
            .ok_or(AccountError::UnknownAccount)
    }

    /// Capability grant set of the currently logged-in identity; this is what
    /// future enforcement points consult for user-scoped policy.
    pub fn active_capabilities(&self) -> Result<u32, AccountError> {
        let claim = self.active.ok_or(AccountError::NotLoggedIn)?;
        self.capabilities_of(claim.account_id)
    }

    fn account_index_of_id(&self, id: u32) -> Option<usize> {
        (0..self.count).find(|index| self.accounts[*index].id == id)
    }

    fn record_ownership(&mut self, index: usize, session_id: u32) -> Result<(), AccountError> {
        let account = &mut self.accounts[index];
        if account.owned_count >= MAX_OWNED_SESSIONS {
            return Err(AccountError::CapacityExceeded);
        }
        if account.owned_sessions[..account.owned_count].contains(&session_id) {
            return Err(AccountError::SessionAlreadyClaimed);
        }
        account.owned_sessions[account.owned_count] = session_id;
        account.owned_count += 1;
        Ok(())
    }

    fn release_ownership(&mut self, index: usize, session_id: u32) {
        let account = &mut self.accounts[index];
        if let Some(position) = account.owned_sessions[..account.owned_count]
            .iter()
            .position(|owned| *owned == session_id)
        {
            account.owned_sessions[position] = account.owned_sessions[account.owned_count - 1];
            account.owned_count -= 1;
        }
    }
}

impl Default for AccountStore {
    fn default() -> Self {
        Self::new()
    }
}

fn copy_field<const N: usize>(source: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    out[..source.len()].copy_from_slice(source);
    out
}

struct FormatBuf<'a> {
    bytes: &'a mut [u8],
    len: usize,
    overflowed: bool,
}

impl<'a> FormatBuf<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            len: 0,
            overflowed: false,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.bytes.len() {
            self.bytes[self.len] = byte;
            self.len += 1;
        } else {
            self.overflowed = true;
        }
    }

    fn push_bytes(&mut self, extra: &[u8]) {
        for byte in extra {
            self.push(*byte);
        }
    }
}

/// Serialize the store as text lines:
/// `account=<id>,<name>,<display>,<salt hex16>,<hash hex16>,<caps hex8>`
/// Only accounts are persisted; the active login claim is runtime state.
pub fn format_store(store: &AccountStore, buffer: &mut [u8]) -> Result<usize, AccountError> {
    let mut cursor = FormatBuf::new(buffer);
    for index in 0..store.count {
        let account = &store.accounts[index];
        cursor.push_bytes(b"account=");
        write_decimal(&mut cursor, account.id as u64);
        cursor.push(b',');
        cursor.push_bytes(&account.name[..account.name_len]);
        cursor.push(b',');
        cursor.push_bytes(&account.display[..account.display_len]);
        cursor.push(b',');
        write_hex(&mut cursor, account.salt, 16);
        cursor.push(b',');
        write_hex(&mut cursor, account.auth_hash, 16);
        cursor.push(b',');
        write_hex(&mut cursor, account.capabilities as u64, 8);
        cursor.push(b'\n');
    }
    if cursor.overflowed {
        return Err(AccountError::CapacityExceeded);
    }
    Ok(cursor.len)
}

fn write_decimal(cursor: &mut FormatBuf, mut value: u64) {
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while count > 0 {
        count -= 1;
        cursor.push(digits[count]);
    }
}

const HEX: [u8; 16] = *b"0123456789abcdef";

fn write_hex(cursor: &mut FormatBuf, value: u64, width: usize) {
    for shift in (0..width).rev() {
        let nibble = ((value >> (shift * 4)) & 0xF) as usize;
        cursor.push(HEX[nibble]);
    }
}

fn parse_hex(text: &[u8]) -> Result<u64, AccountError> {
    let mut value = 0u64;
    for byte in text {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return Err(AccountError::StoreCorrupt),
        };
        value = value
            .checked_mul(16)
            .and_then(|v| v.checked_add(digit as u64))
            .ok_or(AccountError::StoreCorrupt)?;
    }
    Ok(value)
}

/// Parse text previously written by `format_store`. Missing file content
/// (empty text) yields an empty store so first boot seeds defaults.
pub fn parse_store(text: &str) -> Result<AccountStore, AccountError> {
    let mut store = AccountStore::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some(body) = line.strip_prefix("account=") else {
            continue;
        };
        let fields: [&[u8]; 6] = match split_fields(body.as_bytes()) {
            Ok(fields) => fields,
            Err(_) => return Err(AccountError::StoreCorrupt),
        };
        let id = match parse_u32(fields[0]) {
            Ok(id) => id,
            Err(_) => return Err(AccountError::StoreCorrupt),
        };
        if fields[1].is_empty() || fields[1].len() > MAX_NAME || fields[2].len() > MAX_DISPLAY {
            return Err(AccountError::StoreCorrupt);
        }
        if store.count >= MAX_ACCOUNTS {
            return Err(AccountError::CapacityExceeded);
        }
        let slot = store.count;
        store.accounts[slot] = Account {
            id,
            name_len: fields[1].len(),
            name: copy_field::<MAX_NAME>(fields[1]),
            display_len: fields[2].len(),
            display: copy_field::<MAX_DISPLAY>(fields[2]),
            salt: parse_hex(fields[3])?,
            auth_hash: parse_hex(fields[4])?,
            capabilities: parse_hex(fields[5])? as u32,
            owned_sessions: [0; MAX_OWNED_SESSIONS],
            owned_count: 0,
        };
        store.count += 1;
        if id >= store.next_id {
            store.next_id = id + 1;
        }
    }
    Ok(store)
}

fn split_fields(body: &[u8]) -> Result<[&[u8]; 6], AccountError> {
    let mut fields: [&[u8]; 6] = [b"", b"", b"", b"", b"", b""];
    let mut count = 0usize;
    let mut start = 0usize;
    for (position, byte) in body.iter().enumerate() {
        if *byte == b',' {
            if count >= 6 {
                return Err(AccountError::StoreCorrupt);
            }
            fields[count] = &body[start..position];
            count += 1;
            start = position + 1;
        }
    }
    if count != 5 {
        return Err(AccountError::StoreCorrupt);
    }
    fields[5] = &body[start..];
    Ok(fields)
}

fn parse_u32(text: &[u8]) -> Result<u32, AccountError> {
    if text.is_empty() {
        return Err(AccountError::StoreCorrupt);
    }
    let mut value = 0u64;
    for byte in text {
        if !byte.is_ascii_digit() {
            return Err(AccountError::StoreCorrupt);
        }
        value = value * 10 + (*byte - b'0') as u64;
    }
    u32::try_from(value).map_err(|_| AccountError::StoreCorrupt)
}

/// Pack `bytes` into wire words (big-endian, zero padded). Returns the number
/// of words written.
pub fn pack_bytes(out: &mut [u64], bytes: &[u8]) -> usize {
    let words = bytes.len().div_ceil(8);
    for (index, word) in out.iter_mut().enumerate().take(words) {
        let mut value = 0u64;
        for slot in 0..8 {
            let position = index * 8 + slot;
            let byte = if position < bytes.len() {
                bytes[position]
            } else {
                0
            };
            value |= (byte as u64) << (56 - slot * 8);
        }
        *word = value;
    }
    words
}

/// Unpack `byte_len` bytes previously written by `pack_bytes` into `out`.
pub fn unpack_bytes(words: &[u64], byte_len: usize, out: &mut [u8]) -> Result<(), AccountError> {
    if byte_len > out.len() || byte_len > words.len() * 8 {
        return Err(AccountError::InvalidArgument);
    }
    for position in 0..byte_len {
        let word = words[position / 8];
        let shift = 56 - (position % 8) * 8;
        out[position] = ((word >> shift) & 0xFF) as u8;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADMIN_ALL: u32 = capability::ADMIN_DEFAULT;
    const OPERATOR_SET: u32 = capability::OPERATOR_DEFAULT;

    #[test]
    fn hash_is_deterministic_and_secret_sensitive() {
        let salt = fnv1a64(b"account-service/salt") ^ 7;
        let first = derive_auth_hash(salt, b"hunter2");
        let again = derive_auth_hash(salt, b"hunter2");
        assert_eq!(first, again);
        assert_ne!(first, derive_auth_hash(salt, b"hunter3"));
        assert_ne!(first, derive_auth_hash(salt ^ 1, b"hunter2"));
    }

    #[test]
    fn created_account_verifies_credentials() {
        let mut store = AccountStore::new();
        let id = store
            .create_account("paul", "Paul H", b"secret", false)
            .expect("create");
        assert_eq!(id, 1);
        assert!(store.check_credentials("paul", b"secret").is_ok());
        assert_eq!(
            store.check_credentials("paul", b"wrong"),
            Err(AccountError::BadCredentials)
        );
        assert_eq!(
            store.check_credentials("nobody", b"secret"),
            Err(AccountError::UnknownAccount)
        );
    }

    #[test]
    fn create_rejects_duplicates_and_capacity_overrun() {
        let mut store = AccountStore::new();
        assert!(store.create_account("a", "A", b"pw", true).is_ok());
        assert_eq!(
            store.create_account("a", "A2", b"pw2", true),
            Err(AccountError::DuplicateName)
        );
        let mut full = AccountStore::new();
        for index in 0..MAX_ACCOUNTS {
            let name = [b'a' + index as u8, 0];
            let _ =
                full.create_account(core::str::from_utf8(&name[..1]).unwrap(), "", b"pw", false);
        }
        assert_eq!(
            full.create_account("zzz", "", b"pw", false),
            Err(AccountError::CapacityExceeded)
        );
        assert_eq!(
            full.create_account("", "", b"pw", false),
            Err(AccountError::InvalidArgument)
        );
    }

    #[test]
    fn login_state_machine_happy_path() {
        let mut store = AccountStore::seed_defaults();
        let claim = store.login("admin", b"admin", 1).expect("login");
        assert_eq!(claim.account_id, 1);
        assert_eq!(claim.session_id, 1);
        assert_eq!(store.active, Some(claim));
        assert_eq!(
            store.logout(1).expect("logout"),
            Claim {
                account_id: 1,
                session_id: 1,
            }
        );
        assert_eq!(store.active, None);
        assert_eq!(store.logout(1), Err(AccountError::NotLoggedIn));
    }

    #[test]
    fn login_rejects_bad_password_double_login_and_bad_session() {
        let mut store = AccountStore::seed_defaults();
        assert_eq!(
            store.login("admin", b"nope", 1),
            Err(AccountError::BadCredentials)
        );
        assert_eq!(store.active, None);
        assert!(store.login("admin", b"admin", 1).is_ok());
        assert_eq!(
            store.login("operator", b"operator", 2),
            Err(AccountError::AlreadyLoggedIn)
        );
        assert_eq!(
            store.login("admin", b"admin", 0),
            Err(AccountError::InvalidArgument)
        );
    }

    #[test]
    fn ownership_tracks_claimed_sessions_across_logout() {
        let mut store = AccountStore::seed_defaults();
        let operator = store.find_by_name("operator").unwrap();
        let operator_id = operator.id;
        assert!(store.login("operator", b"operator", 3).is_ok());
        let index = store.account_index(b"operator").unwrap();
        assert_eq!(store.accounts[index].owned_sessions[0], 3);
        store.logout(3).expect("logout");
        let index = store.account_index(b"operator").unwrap();
        assert_eq!(store.accounts[index].owned_count, 0);
        let _ = operator_id;
    }

    #[test]
    fn switch_user_moves_identity_between_sessions() {
        let mut store = AccountStore::seed_defaults();
        assert_eq!(store.switch_user(9), Err(AccountError::NotLoggedIn));
        store.login("admin", b"admin", 4).expect("login");
        let record = store.switch_user(9).expect("switch");
        assert_eq!(
            record,
            SwitchRecord {
                account_id: 1,
                from_session: 4,
                to_session: 9,
            }
        );
        assert_eq!(
            store.active,
            Some(Claim {
                account_id: 1,
                session_id: 9,
            })
        );
        let index = store.account_index(b"admin").unwrap();
        assert_eq!(store.accounts[index].owned_count, 1);
        assert_eq!(store.accounts[index].owned_sessions[0], 9);
        assert_eq!(
            store.switch_user(9),
            Err(AccountError::SessionAlreadyClaimed)
        );
        store.logout(9).expect("logout");
    }

    #[test]
    fn policy_defaults_match_account_class() {
        assert_eq!(default_capabilities(true), ADMIN_ALL);
        assert_eq!(default_capabilities(false), OPERATOR_SET);
        assert_eq!(OPERATOR_SET & capability::PACKAGE_INSTALL, 0);
        assert_eq!(OPERATOR_SET & capability::DEVELOPER_TOOLS, 0);

        let store = AccountStore::seed_defaults();
        assert_eq!(store.capabilities_of(1), Ok(ADMIN_ALL));
        assert_eq!(store.capabilities_of(2), Ok(OPERATOR_SET));
        assert_eq!(store.capabilities_of(99), Err(AccountError::UnknownAccount));
    }

    #[test]
    fn active_capabilities_follow_the_login_claim() {
        let mut store = AccountStore::seed_defaults();
        assert_eq!(store.active_capabilities(), Err(AccountError::NotLoggedIn));
        store.login("operator", b"operator", 2).expect("login");
        assert_eq!(store.active_capabilities(), Ok(OPERATOR_SET));
    }

    #[test]
    fn store_roundtrips_through_text() {
        let mut store = AccountStore::new();
        store
            .create_account("admin", "Administrator", b"s3cret", true)
            .unwrap();
        store
            .create_account("op", "Operator", b"hunter2", false)
            .unwrap();

        let mut buffer = [0u8; 1024];
        let used = format_store(&store, &mut buffer).expect("format");
        let text = core::str::from_utf8(&buffer[..used]).unwrap();

        let parsed = parse_store(text).expect("parse");
        assert_eq!(parsed.count, 2);
        assert_eq!(parsed.next_id, 3);
        assert_eq!(parsed.accounts[0].salt, store.accounts[0].salt);
        assert_eq!(parsed.accounts[0].auth_hash, store.accounts[0].auth_hash);
        assert_eq!(parsed.accounts[1].capabilities, OPERATOR_SET);
        // Credentials still verify after a roundtrip.
        assert!(parsed.check_credentials("admin", b"s3cret").is_ok());
        assert!(parsed.check_credentials("op", b"hunter2").is_ok());
        // Active claim is runtime-only and never persisted.
        assert_eq!(parsed.active, None);
    }

    #[test]
    fn parse_empty_text_yields_empty_store_and_garbage_is_corrupt() {
        let store = parse_store("").expect("empty");
        assert_eq!(store.count, 0);

        let mut buffer = [0u8; 256];
        let mut store = AccountStore::new();
        store.create_account("x", "X", b"pw", false).unwrap();
        let used = format_store(&store, &mut buffer).unwrap();
        buffer[used - 1] = b'!'; // clobber final caps hex digit
        assert_eq!(
            parse_store(core::str::from_utf8(&buffer[..used]).unwrap()),
            Err(AccountError::StoreCorrupt)
        );
    }

    #[test]
    fn wire_pack_roundtrip() {
        let mut words = [0u64; 8];
        let used = pack_bytes(&mut words, b"operator");
        assert_eq!(used, 1);
        let mut out = [0u8; MAX_NAME];
        unpack_bytes(&words, 8, &mut out).expect("unpack");
        assert_eq!(&out[..8], b"operator");

        assert_eq!(pack_bytes(&mut words, b"123456789abc"), 2);
        unpack_bytes(&words, 12, &mut out).expect("unpack");
        assert_eq!(&out[..12], b"123456789abc");
        assert_eq!(
            unpack_bytes(&words, 100, &mut out),
            Err(AccountError::InvalidArgument)
        );
    }
}
