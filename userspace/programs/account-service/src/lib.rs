//! Account store, login/identity state machine, and per-account policy
//! defaults for the account service. Pure logic plus the service's own wire
//! tags and byte packing helpers, shared between the `no_std` service binary
//! and host unit tests.
//!
//! Credential hashing note: the store supports two record algorithms.
//! `KDF_PBKDF2_SHA512` records derive credential hashes with PBKDF2-HMAC-
//! SHA-512 (RFC 8018) from `serviceos-crypto`, a real password KDF; new
//! accounts and password changes always use it. Legacy `KDF_LEGACY_FNV`
//! records keep the historical salted, stretched FNV-1a derivation
//! (`derive_auth_hash`, honestly non-cryptographic) for backwards
//! compatibility; they are transparently UPGRADED to PBKDF2 on the next
//! successful login. Per-account 128-bit salts are stored alongside the
//! derived hash, shadow-file style. Salt honesty: no kernel RNG exists yet,
//! so salts are boot-local substitutes (SHA-512 over the account name, a
//! boot tick captured at startup, and a per-creation counter) — unique-ish,
//! NOT cryptographically random; see `pbkdf2_salt`.

#![cfg_attr(not(test), no_std)]

pub const MAX_ACCOUNTS: usize = 8;
pub const MAX_NAME: usize = 32;
pub const MAX_DISPLAY: usize = 48;
pub const MAX_OWNED_SESSIONS: usize = 4;

/// Store file path, relative to the storage root granted at startup.
pub const ACCOUNTS_PATH: &str = "state/account/accounts.cfg";

/// KDF stretching rounds for `derive_auth_hash`.
pub const KDF_ROUNDS: u32 = 8;

/// Credential record algorithms. `0` is the legacy FNV placeholder; `1` is
/// PBKDF2-HMAC-SHA-512 (RFC 8018). Persisted as the `pbkdf2-sha512` text
/// marker; anything else in that slot is a corrupt store.
pub const KDF_LEGACY_FNV: u8 = 0;
pub const KDF_PBKDF2_SHA512: u8 = 1;

/// Codec text marker for PBKDF2-HMAC-SHA-512 records.
pub const KDF_PBKDF2_SHA512_NAME: &[u8] = b"pbkdf2-sha512";

/// PBKDF2 salt length in bytes (128-bit).
pub const PBKDF2_SALT_BYTES: usize = 16;
/// PBKDF2 derived-hash length in bytes (one full SHA-512 block of output).
pub const PBKDF2_HASH_BYTES: usize = 64;

/// PBKDF2-HMAC-SHA-512 rounds for new and upgraded credential records.
///
/// Pinned from a one-time host measurement (x86_64, release profile,
/// /tmp/kdf-bench): 100k rounds ≈ 115ms (≈0.575ms per 10k rounds), so a
/// login on x86 KVM stays far under the ~1s operator budget. TCG honesty:
/// aarch64 virt under TCG stretches per-instruction cost ~10-20x, so the
/// same verify can take seconds there — that is the honest cost of a real
/// KDF on emulation, not a regression.
pub const PBKDF2_LOGIN_ITERATIONS: u32 = 100_000;

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
    /// Set-password contract: verify the old secret, re-derive the record as
    /// PBKDF2 with a fresh salt. Request packs name, old secret, new secret
    /// (same string layout as CREATE minus display/admin); reply carries the
    /// account id on success.
    pub const SET_PASSWORD_REQUEST: u32 = 0x21D;
    pub const SET_PASSWORD_REPLY: u32 = 0x21E;
    /// Verify-password contract (additive, used by network-service's sshd):
    /// name + secret (LOGIN layout minus the session id), answer words[0] =
    /// status code (0 = Ok) and words[1] = 1 when the credentials verify,
    /// 0 otherwise. Read-only: no claim is created, no record is upgraded —
    /// upgrades happen on the interactive LOGIN path.
    pub const VERIFY_PASSWORD_REQUEST: u32 = 0x21F;
    pub const VERIFY_PASSWORD_REPLY: u32 = 0x220;
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
    /// Legacy FNV salt (meaningful only for KDF_LEGACY_FNV records; 0 for
    /// PBKDF2 records).
    pub salt: u64,
    /// Legacy FNV derived hash (same condition as `salt`).
    pub auth_hash: u64,
    pub capabilities: u32,
    pub owned_sessions: [u32; MAX_OWNED_SESSIONS],
    pub owned_count: usize,
    /// Credential algorithm: KDF_LEGACY_FNV or KDF_PBKDF2_SHA512.
    pub kdf: u8,
    /// PBKDF2 round count (0 for legacy records).
    pub kdf_iters: u32,
    /// PBKDF2 128-bit salt.
    pub pbkdf2_salt: [u8; PBKDF2_SALT_BYTES],
    /// PBKDF2 64-byte derived hash.
    pub pbkdf2_hash: [u8; PBKDF2_HASH_BYTES],
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
    kdf: KDF_LEGACY_FNV,
    kdf_iters: 0,
    pbkdf2_salt: [0; PBKDF2_SALT_BYTES],
    pbkdf2_hash: [0; PBKDF2_HASH_BYTES],
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

/// Derive a fresh 128-bit PBKDF2 salt from guest-local entropy substitutes:
/// SHA-512 over (domain constant, account name, boot tick, per-creation
/// counter, account id), truncated to 16 bytes — the same boot-local
/// substitute shape package-service uses for Ed25519 seeds.
///
/// HONEST LIMITS: this kernel exposes no hardware RNG yet and the boot tick
/// may stand still on some builds (the setup-wizard seeds records with
/// tick 0), so salts are UNIQUE-ISH (distinct across names/ids/counters),
/// NOT cryptographically random. They defend against equal-hash lookup and
/// per-account stretching, not against an attacker who can recompute the
/// whole derivation table offline.
pub fn pbkdf2_salt(name: &[u8], tick: u64, counter: u64, id: u32) -> [u8; PBKDF2_SALT_BYTES] {
    let mut block = [0u8; 64];
    // Layout: [0..8) counter, [8..16) tick, [16..20) id, [20..52) name (<=32).
    block[..8].copy_from_slice(&counter.to_le_bytes());
    block[8..16].copy_from_slice(&tick.to_le_bytes());
    block[16..20].copy_from_slice(&id.to_le_bytes());
    let name_len = name.len().min(32);
    block[20..20 + name_len].copy_from_slice(&name[..name_len]);
    let digest = serviceos_crypto::sha512::digest(&[b"account-service/pbkdf2-salt", &block]);
    let mut salt = [0u8; PBKDF2_SALT_BYTES];
    salt.copy_from_slice(&digest[..PBKDF2_SALT_BYTES]);
    salt
}

/// PBKDF2-HMAC-SHA-512 verification digest for a record's (salt, iterations).
pub fn pbkdf2_derive(
    salt: &[u8; PBKDF2_SALT_BYTES],
    iterations: u32,
    secret: &[u8],
) -> [u8; PBKDF2_HASH_BYTES] {
    let mut out = [0u8; PBKDF2_HASH_BYTES];
    serviceos_crypto::pbkdf2::pbkdf2_hmac_sha512(secret, salt, iterations, &mut out);
    out
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
    /// Per-creation salt counter (mixed into `pbkdf2_salt`).
    pub salt_counter: u64,
    /// Boot tick captured at service start (0 when unset, e.g. wizard-seeded
    /// stores); mixed into `pbkdf2_salt`.
    pub salt_tick: u64,
}

impl AccountStore {
    pub const fn new() -> Self {
        Self {
            accounts: [EMPTY_ACCOUNT; MAX_ACCOUNTS],
            count: 0,
            next_id: 1,
            active: None,
            salt_counter: 0,
            salt_tick: 0,
        }
    }

    /// Bootstrap accounts seeded when no persisted store exists. Placeholder
    /// credentials kept on the LEGACY FNV KDF: seeding must cost nothing at
    /// boot, and the migration story is transparent — the first successful
    /// login upgrades each record to PBKDF2-HMAC-SHA-512. Changing them is
    /// expected before real use.
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

    /// Create an account with the default grant set for its class. New
    /// credentials ALWAYS use PBKDF2-HMAC-SHA-512 with 128-bit boot-local
    /// salts (see `pbkdf2_salt`) and `PBKDF2_LOGIN_ITERATIONS` rounds.
    /// Returns the new account id.
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
        // Distinct accounts never share a salt: name, id, boot tick, and a
        // per-creation counter all mix into the salt derivation.
        let salt = pbkdf2_salt(name.as_bytes(), self.salt_tick, self.salt_counter, id);
        self.salt_counter += 1;
        self.accounts[slot] = Account {
            id,
            name_len: name.len(),
            name: copy_field::<MAX_NAME>(name.as_bytes()),
            display_len: display.len(),
            display: copy_field::<MAX_DISPLAY>(display.as_bytes()),
            salt: 0,
            auth_hash: 0,
            capabilities: default_capabilities(is_admin),
            owned_sessions: [0; MAX_OWNED_SESSIONS],
            owned_count: 0,
            kdf: KDF_PBKDF2_SHA512,
            kdf_iters: PBKDF2_LOGIN_ITERATIONS,
            pbkdf2_salt: salt,
            pbkdf2_hash: pbkdf2_derive(&salt, PBKDF2_LOGIN_ITERATIONS, secret),
        };
        self.count += 1;
        self.next_id += 1;
        Ok(id)
    }

    /// Legacy-FNV record seeding for migration tests and tooling; not used
    /// by the creation path (new credentials are always PBKDF2).
    pub fn seed_legacy_account(
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
        // Historical derivation: fixed domain constant mixed with the name
        // so distinct accounts never shared a salt.
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
            kdf: KDF_LEGACY_FNV,
            kdf_iters: 0,
            pbkdf2_salt: [0; PBKDF2_SALT_BYTES],
            pbkdf2_hash: [0; PBKDF2_HASH_BYTES],
        };
        self.count += 1;
        self.next_id += 1;
        Ok(id)
    }

    fn verify(&self, index: usize, secret: &[u8]) -> bool {
        let account = &self.accounts[index];
        match account.kdf {
            KDF_PBKDF2_SHA512 => {
                let derived = pbkdf2_derive(&account.pbkdf2_salt, account.kdf_iters, secret);
                serviceos_crypto::pbkdf2::ct_eq(&derived, &account.pbkdf2_hash)
            }
            // Legacy FNV path; equality compare is fine here — the old
            // derivation was never constant-time to begin with, and this
            // branch exists only until the record is upgraded.
            _ => derive_auth_hash(account.salt, secret) == account.auth_hash,
        }
    }

    /// Re-derive a credential record as PBKDF2-HMAC-SHA-512 with a fresh
    /// salt (upgrade-on-login and password changes).
    fn rederive_pbkdf2(&mut self, index: usize, secret: &[u8]) {
        let id = self.accounts[index].id;
        let name_len = self.accounts[index].name_len;
        let mut name = [0u8; MAX_NAME];
        name[..name_len].copy_from_slice(&self.accounts[index].name[..name_len]);
        let salt = pbkdf2_salt(&name[..name_len], self.salt_tick, self.salt_counter, id);
        self.salt_counter += 1;
        let account = &mut self.accounts[index];
        account.salt = 0;
        account.auth_hash = 0;
        account.kdf = KDF_PBKDF2_SHA512;
        account.kdf_iters = PBKDF2_LOGIN_ITERATIONS;
        account.pbkdf2_salt = salt;
        account.pbkdf2_hash = pbkdf2_derive(&salt, PBKDF2_LOGIN_ITERATIONS, secret);
    }

    /// Change an account's password: verify `old_secret`, then re-derive the
    /// record as PBKDF2 with a fresh salt and `new_secret`. Returns the
    /// account index.
    pub fn set_password(
        &mut self,
        name: &str,
        old_secret: &[u8],
        new_secret: &[u8],
    ) -> Result<usize, AccountError> {
        if new_secret.is_empty() {
            return Err(AccountError::InvalidArgument);
        }
        let Some(index) = self.account_index(name.as_bytes()) else {
            return Err(AccountError::UnknownAccount);
        };
        if !self.verify(index, old_secret) {
            return Err(AccountError::BadCredentials);
        }
        self.rederive_pbkdf2(index, new_secret);
        Ok(index)
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
    /// of a session-service session id. Legacy FNV credential records are
    /// transparently UPGRADED to PBKDF2-HMAC-SHA-512 on successful login
    /// (write-through persistence is the caller's job); the returned bool
    /// reports whether an upgrade happened so the wire layer can persist.
    pub fn login(
        &mut self,
        name: &str,
        secret: &[u8],
        session_id: u32,
    ) -> Result<(Claim, bool), AccountError> {
        if session_id == 0 {
            return Err(AccountError::InvalidArgument);
        }
        if self.active.is_some() {
            return Err(AccountError::AlreadyLoggedIn);
        }
        let index = self.check_credentials(name, secret)?;
        let mut upgraded = false;
        if self.accounts[index].kdf == KDF_LEGACY_FNV {
            self.rederive_pbkdf2(index, secret);
            upgraded = true;
        }
        let claim = Claim {
            account_id: self.accounts[index].id,
            session_id,
        };
        self.record_ownership(index, session_id)?;
        self.active = Some(claim);
        Ok((claim, upgraded))
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
/// for legacy FNV records, plus ADDITIVE trailing fields for PBKDF2 records:
/// `account=<id>,<name>,<display>,0,0,<caps hex8>,pbkdf2-sha512,<iters>,<salt
/// hex32>,<hash hex128>`. Only accounts are persisted; the active login
/// claim is runtime state. Old-format lines keep their exact shape so the
/// historical parser semantics stay valid.
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
        match account.kdf {
            KDF_PBKDF2_SHA512 => {
                // Legacy slots stay zero for PBKDF2 records; real material
                // lives in the additive trailing fields.
                write_hex(&mut cursor, 0, 16);
                cursor.push(b',');
                write_hex(&mut cursor, 0, 16);
                cursor.push(b',');
                write_hex(&mut cursor, account.capabilities as u64, 8);
                cursor.push(b',');
                cursor.push_bytes(KDF_PBKDF2_SHA512_NAME);
                cursor.push(b',');
                write_decimal(&mut cursor, account.kdf_iters as u64);
                cursor.push(b',');
                push_hex_bytes(&mut cursor, &account.pbkdf2_salt);
                cursor.push(b',');
                push_hex_bytes(&mut cursor, &account.pbkdf2_hash);
            }
            _ => {
                write_hex(&mut cursor, account.salt, 16);
                cursor.push(b',');
                write_hex(&mut cursor, account.auth_hash, 16);
                cursor.push(b',');
                write_hex(&mut cursor, account.capabilities as u64, 8);
            }
        }
        cursor.push(b'\n');
    }
    if cursor.overflowed {
        return Err(AccountError::CapacityExceeded);
    }
    Ok(cursor.len)
}

/// Lowercase hex of a byte slice.
fn push_hex_bytes(cursor: &mut FormatBuf, bytes: &[u8]) {
    for byte in bytes {
        cursor.push(HEX[(byte >> 4) as usize]);
        cursor.push(HEX[(byte & 0xF) as usize]);
    }
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
/// (empty text) yields an empty store so first boot seeds defaults. Both
/// record shapes are accepted: legacy 6-field FNV lines and the additive
/// 10-field PBKDF2 lines; any other field count or algorithm marker is a
/// corrupt store.
pub fn parse_store(text: &str) -> Result<AccountStore, AccountError> {
    let mut store = AccountStore::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some(body) = line.strip_prefix("account=") else {
            continue;
        };
        let mut fields: [&[u8]; MAX_FIELDS] = [b""; MAX_FIELDS];
        let field_count = match split_fields(body.as_bytes(), &mut fields) {
            Ok(count) => count,
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
        let mut account = Account {
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
            kdf: KDF_LEGACY_FNV,
            kdf_iters: 0,
            pbkdf2_salt: [0; PBKDF2_SALT_BYTES],
            pbkdf2_hash: [0; PBKDF2_HASH_BYTES],
        };
        if field_count == LEGACY_FIELD_COUNT {
            // Legacy FNV record: nothing further to parse.
        } else if field_count == MAX_FIELDS && fields[6] == KDF_PBKDF2_SHA512_NAME {
            account.kdf = KDF_PBKDF2_SHA512;
            account.kdf_iters = parse_u32(fields[7])?;
            account.pbkdf2_salt = parse_hex_into::<PBKDF2_SALT_BYTES>(fields[8])?;
            account.pbkdf2_hash = parse_hex_into::<PBKDF2_HASH_BYTES>(fields[9])?;
        } else {
            return Err(AccountError::StoreCorrupt);
        }
        store.accounts[slot] = account;
        store.count += 1;
        if id >= store.next_id {
            store.next_id = id + 1;
        }
    }
    Ok(store)
}

/// Legacy lines carry exactly 6 fields; PBKDF2 lines append algorithm,
/// iteration count, 128-bit salt (32 hex), and 64-byte hash (128 hex).
const LEGACY_FIELD_COUNT: usize = 6;
const MAX_FIELDS: usize = 10;

fn split_fields<'a>(
    body: &'a [u8],
    fields: &mut [&'a [u8]; MAX_FIELDS],
) -> Result<usize, AccountError> {
    let mut count = 0usize;
    let mut start = 0usize;
    for (position, byte) in body.iter().enumerate() {
        if *byte == b',' {
            if count >= MAX_FIELDS {
                return Err(AccountError::StoreCorrupt);
            }
            fields[count] = &body[start..position];
            count += 1;
            start = position + 1;
        }
    }
    if count >= MAX_FIELDS {
        return Err(AccountError::StoreCorrupt);
    }
    fields[count] = &body[start..];
    count += 1;
    Ok(count)
}

/// Parse `2 * N` lowercase hex chars into an N-byte array.
fn parse_hex_into<const N: usize>(text: &[u8]) -> Result<[u8; N], AccountError> {
    if text.len() != N * 2 {
        return Err(AccountError::StoreCorrupt);
    }
    let mut out = [0u8; N];
    for index in 0..N {
        let high = match text[index * 2] {
            digit @ b'0'..=b'9' => digit - b'0',
            digit @ b'a'..=b'f' => digit - b'a' + 10,
            _ => return Err(AccountError::StoreCorrupt),
        };
        let low = match text[index * 2 + 1] {
            digit @ b'0'..=b'9' => digit - b'0',
            digit @ b'a'..=b'f' => digit - b'a' + 10,
            _ => return Err(AccountError::StoreCorrupt),
        };
        out[index] = (high << 4) | low;
    }
    Ok(out)
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
        let (claim, _upgraded) = store.login("admin", b"admin", 1).expect("login");
        assert_eq!(claim.account_id, 1);
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

    /// Test speedup: shrink a PBKDF2 record to 128 rounds using the known
    /// secret. The verify path always honors the record's own iteration
    /// count, so flow tests at reduced cost still exercise the real codec
    /// and verification logic; the full pinned count is covered separately.
    fn shrink_iters(store: &mut AccountStore, index: usize, secret: &[u8]) {
        assert_eq!(store.accounts[index].kdf, KDF_PBKDF2_SHA512);
        let salt = store.accounts[index].pbkdf2_salt;
        store.accounts[index].kdf_iters = 128;
        store.accounts[index].pbkdf2_hash = pbkdf2_derive(&salt, 128, secret);
    }

    /// New credentials: create_account produces a PBKDF2 record, verifies
    /// with the real pinned iteration count, rejects the wrong secret, and
    /// round-trips through the additive codec.
    #[test]
    fn pbkdf2_create_verify_and_codec_roundtrip() {
        let mut store = AccountStore::new();
        let id = store
            .create_account("ada", "Ada", b"correct horse", true)
            .expect("create");
        assert_eq!(id, 1);
        assert_eq!(store.accounts[0].kdf, KDF_PBKDF2_SHA512);
        assert_eq!(store.accounts[0].kdf_iters, PBKDF2_LOGIN_ITERATIONS);
        assert_ne!(store.accounts[0].pbkdf2_salt, [0u8; PBKDF2_SALT_BYTES]);
        assert!(store.check_credentials("ada", b"correct horse").is_ok());
        assert_eq!(
            store.check_credentials("ada", b"wrong horse"),
            Err(AccountError::BadCredentials)
        );

        let mut buffer = [0u8; 1024];
        let used = format_store(&store, &mut buffer).expect("format");
        let text = core::str::from_utf8(&buffer[..used]).unwrap();
        assert!(text.contains("pbkdf2-sha512"));
        let parsed = parse_store(text).expect("parse");
        assert_eq!(parsed.count, 1);
        assert_eq!(parsed.accounts[0].kdf, KDF_PBKDF2_SHA512);
        assert_eq!(parsed.accounts[0].kdf_iters, PBKDF2_LOGIN_ITERATIONS);
        assert_eq!(
            parsed.accounts[0].pbkdf2_salt,
            store.accounts[0].pbkdf2_salt
        );
        assert_eq!(
            parsed.accounts[0].pbkdf2_hash,
            store.accounts[0].pbkdf2_hash
        );
        assert_eq!(parsed.accounts[0].salt, 0);
        assert_eq!(parsed.accounts[0].auth_hash, 0);
    }

    /// Legacy records verify via the old path with NO upgrade until a
    /// successful login, then transparently upgrade to PBKDF2 and keep
    /// working on the next login.
    #[test]
    fn legacy_record_upgrades_on_successful_login_only() {
        let mut store = AccountStore::new();
        store
            .seed_legacy_account("root", "Root", b"old-secret", true)
            .expect("seed");
        assert_eq!(store.accounts[0].kdf, KDF_LEGACY_FNV);
        let original_hash = store.accounts[0].auth_hash;

        // Wrong password: verify fails, record untouched.
        assert_eq!(
            store.check_credentials("root", b"nope"),
            Err(AccountError::BadCredentials)
        );
        assert_eq!(store.accounts[0].kdf, KDF_LEGACY_FNV);
        assert_eq!(store.accounts[0].auth_hash, original_hash);
        assert_eq!(
            store.login("root", b"nope", 1),
            Err(AccountError::BadCredentials)
        );
        assert_eq!(store.accounts[0].kdf, KDF_LEGACY_FNV);
        assert_eq!(store.accounts[0].auth_hash, original_hash);

        // Successful login upgrades the record and reports the upgrade.
        let (_claim, upgraded) = store.login("root", b"old-secret", 1).expect("login");
        assert!(upgraded);
        assert_eq!(store.accounts[0].kdf, KDF_PBKDF2_SHA512);
        assert_eq!(store.accounts[0].kdf_iters, PBKDF2_LOGIN_ITERATIONS);
        assert_ne!(store.accounts[0].pbkdf2_salt, [0u8; PBKDF2_SALT_BYTES]);
        assert_eq!(store.accounts[0].salt, 0);
        assert_eq!(store.accounts[0].auth_hash, 0);

        // Old secret still verifies through the PBKDF2 path; logout then
        // re-login works and reports no second upgrade.
        shrink_iters(&mut store, 0, b"old-secret");
        store.logout(1).expect("logout");
        let (_claim, upgraded_again) = store.login("root", b"old-secret", 2).expect("relogin");
        assert!(!upgraded_again);
    }

    /// set_password: re-derives the record as PBKDF2 with a fresh salt only
    /// when the old secret verifies; failure leaves the record untouched.
    #[test]
    fn set_password_requires_old_secret_and_replaces_credential() {
        let mut store = AccountStore::new();
        store
            .seed_legacy_account("op", "Op", b"first", false)
            .expect("seed");
        // Upgrade the record through the real login path, then shrink.
        store.login("op", b"first", 1).expect("login");
        store.logout(1).expect("logout");
        shrink_iters(&mut store, 0, b"first");
        let before = store.accounts[0];

        // Wrong old secret: nothing changes.
        assert_eq!(
            store.set_password("op", b"wrong", b"second"),
            Err(AccountError::BadCredentials)
        );
        assert_eq!(store.accounts[0].pbkdf2_hash, before.pbkdf2_hash);
        assert_eq!(store.accounts[0].pbkdf2_salt, before.pbkdf2_salt);

        // Correct old secret: fresh salt, new secret verifies, old fails.
        let index = store.set_password("op", b"first", b"second").expect("set");
        assert_eq!(index, 0);
        assert_eq!(store.accounts[0].kdf, KDF_PBKDF2_SHA512);
        assert_ne!(store.accounts[0].pbkdf2_salt, before.pbkdf2_salt);
        assert!(store.check_credentials("op", b"second").is_ok());
        assert_eq!(
            store.check_credentials("op", b"first"),
            Err(AccountError::BadCredentials)
        );

        // Empty new secret is rejected before any verification work.
        assert_eq!(
            store.set_password("op", b"second", b""),
            Err(AccountError::InvalidArgument)
        );
    }

    /// Codec accepts both record shapes and rejects unknown algorithms and
    /// truncated additive records.
    #[test]
    fn codec_accepts_both_record_shapes_and_flags_corrupt_lines() {
        // Legacy 6-field line: parses as a legacy FNV record.
        let store = parse_store("account=1,root,Root,0badc0ffee000000,1234567890abcdef,ff\n")
            .expect("legacy line");
        assert_eq!(store.count, 1);
        assert_eq!(store.accounts[0].kdf, KDF_LEGACY_FNV);
        assert_eq!(store.accounts[0].salt, 0x0bad_c0ff_ee00_0000);

        // PBKDF2 line with a bogus algorithm marker is corrupt.
        assert_eq!(
            parse_store(
                "account=1,root,Root,0000000000000000,0000000000000000,ff,pbkdf2-sha1,128,00112233445566778899aabbccddeeff,<128 hex>\n"
            ),
            Err(AccountError::StoreCorrupt)
        );

        // Seven fields (truncated additive record) is corrupt.
        assert_eq!(
            parse_store("account=1,root,Root,0000000000000000,0000000000000000,ff,extra\n"),
            Err(AccountError::StoreCorrupt)
        );
    }

    /// Salts stay distinct across ids, ticks, counters, and names, and are
    /// deterministic for identical inputs.
    #[test]
    fn pbkdf2_salts_differ_across_accounts_ticks_and_counters() {
        let base = pbkdf2_salt(b"ada", 0, 0, 1);
        assert_eq!(base, pbkdf2_salt(b"ada", 0, 0, 1));
        assert_ne!(base, pbkdf2_salt(b"ada", 0, 0, 2));
        assert_ne!(base, pbkdf2_salt(b"ada", 7, 0, 1));
        assert_ne!(base, pbkdf2_salt(b"ada", 0, 1, 1));
        assert_ne!(base, pbkdf2_salt(b"bob", 0, 0, 1));
        // The store's counter keeps per-creation salts distinct even for
        // the same name (fresh record after deletion).
        let mut store = AccountStore::new();
        store.salt_tick = 3;
        let first = pbkdf2_salt(b"u", store.salt_tick, store.salt_counter, 1);
        store.salt_counter += 1;
        let second = pbkdf2_salt(b"u", store.salt_tick, store.salt_counter, 1);
        assert_ne!(first, second);
    }
}
