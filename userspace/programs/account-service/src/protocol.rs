//! Wire protocol for the account service's own control channel. Requests
//! carry a reply channel as handles[0]; replies are status-first
//! (`AccountError::to_code`, 0 = Ok) followed by op-specific words.

use serviceos_account_service::{
    AccountError, AccountStore, MAX_DISPLAY, MAX_NAME, SwitchRecord, account_tag, unpack_bytes,
};
use serviceos_userspace_runtime::RawMessage;

pub const MAX_SECRET: usize = 64;

pub struct RequestScratch {
    pub name: [u8; MAX_NAME],
    pub display: [u8; MAX_DISPLAY],
    pub secret: [u8; MAX_SECRET],
}

impl RequestScratch {
    pub fn new() -> Self {
        Self {
            name: [0; MAX_NAME],
            display: [0; MAX_DISPLAY],
            secret: [0; MAX_SECRET],
        }
    }
}

impl Default for RequestScratch {
    fn default() -> Self {
        Self::new()
    }
}

fn field_at(request: &RawMessage, offset: usize) -> Result<(usize, usize), AccountError> {
    let len = *request
        .words
        .get(offset)
        .ok_or(AccountError::InvalidArgument)? as usize;
    let start_word = offset + 1;
    let words = len.div_ceil(8);
    if start_word + words > request.word_count as usize {
        return Err(AccountError::InvalidArgument);
    }
    Ok((len, start_word))
}

fn decode_str(
    request: &RawMessage,
    offset: usize,
    max_len: usize,
    out: &mut [u8],
) -> Result<usize, AccountError> {
    let (len, start_word) = field_at(request, offset)?;
    if len > max_len {
        return Err(AccountError::InvalidArgument);
    }
    unpack_bytes(&request.words[start_word..], len, &mut out[..len])
        .map_err(|_| AccountError::InvalidArgument)?;
    Ok(len)
}

fn str_field<'a>(scratch: &'a [u8], len: usize) -> &'a str {
    // Lengths validated against known-good ASCII inputs at decode time;
    // fall back to empty on non-UTF8 rather than panicking.
    core::str::from_utf8(&scratch[..len]).unwrap_or("")
}

/// Handle one wire request. Returns true when the PERSISTED store changed
/// (account created, password set, or a legacy credential record upgraded to
/// PBKDF2 on login) so the caller can write through to storage; runtime-only
/// state (claims, session ownership) does not count.
pub fn handle_request(
    store: &mut AccountStore,
    request: &RawMessage,
    response: &mut RawMessage,
    scratch: &mut RequestScratch,
) -> bool {
    match request.tag {
        x if x == account_tag::CREATE_REQUEST => {
            response.tag = account_tag::CREATE_REPLY;
            let name_len = match decode_str(request, 0, MAX_NAME, &mut scratch.name) {
                Ok(len) => len,
                Err(error) => return fail(response, error),
            };
            let display_offset = 1 + name_len.div_ceil(8);
            let display_len =
                match decode_str(request, display_offset, MAX_DISPLAY, &mut scratch.display) {
                    Ok(len) => len,
                    Err(error) => return fail(response, error),
                };
            let secret_offset = display_offset + 1 + display_len.div_ceil(8);
            let secret_len =
                match decode_str(request, secret_offset, MAX_SECRET, &mut scratch.secret) {
                    Ok(len) => len,
                    Err(error) => return fail(response, error),
                };
            let admin_index = secret_offset + 1 + secret_len.div_ceil(8);
            let admin = *request.words.get(admin_index).unwrap_or(&0) != 0;
            match store.create_account(
                str_field(&scratch.name, name_len),
                str_field(&scratch.display, display_len),
                &scratch.secret[..secret_len],
                admin,
            ) {
                Ok(id) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = id as u64;
                    true
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == account_tag::LOGIN_REQUEST => {
            response.tag = account_tag::LOGIN_REPLY;
            let name_len = match decode_str(request, 0, MAX_NAME, &mut scratch.name) {
                Ok(l) => l,
                Err(e) => return fail(response, e),
            };
            let secret_offset = 1 + name_len.div_ceil(8);
            let secret_len =
                match decode_str(request, secret_offset, MAX_SECRET, &mut scratch.secret) {
                    Ok(l) => l,
                    Err(e) => return fail(response, e),
                };
            let session_index = secret_offset + 1 + secret_len.div_ceil(8);
            let Some(&session_id) = request.words.get(session_index) else {
                return fail(response, AccountError::InvalidArgument);
            };
            match store.login(
                str_field(&scratch.name, name_len),
                &scratch.secret[..secret_len],
                session_id as u32,
            ) {
                Ok((claim, upgraded)) => {
                    let caps = store.active_capabilities().unwrap_or(0);
                    response.word_count = 4;
                    response.words[0] = 0;
                    response.words[1] = claim.account_id as u64;
                    response.words[2] = claim.session_id as u64;
                    response.words[3] = caps as u64;
                    upgraded
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == account_tag::SET_PASSWORD_REQUEST => {
            response.tag = account_tag::SET_PASSWORD_REPLY;
            let name_len = match decode_str(request, 0, MAX_NAME, &mut scratch.name) {
                Ok(l) => l,
                Err(e) => return fail(response, e),
            };
            let old_offset = 1 + name_len.div_ceil(8);
            let old_len = match decode_str(request, old_offset, MAX_SECRET, &mut scratch.secret) {
                Ok(l) => l,
                Err(e) => return fail(response, e),
            };
            // New secret borrows the display scratch slot: it arrives after
            // the old secret and both are needed only until re-derivation.
            let new_offset = old_offset + 1 + old_len.div_ceil(8);
            let new_len = match decode_str(request, new_offset, MAX_SECRET, &mut scratch.display) {
                Ok(l) => l,
                Err(e) => return fail(response, e),
            };
            match store.set_password(
                str_field(&scratch.name, name_len),
                &scratch.secret[..old_len],
                &scratch.display[..new_len],
            ) {
                Ok(index) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = store.accounts[index].id as u64;
                    true
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == account_tag::LOGOUT_REQUEST => {
            response.tag = account_tag::LOGOUT_REPLY;
            let Some(&session_id) = request.words.first() else {
                return fail(response, AccountError::InvalidArgument);
            };
            match store.logout(session_id as u32) {
                Ok(claim) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = claim.account_id as u64;
                    false
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == account_tag::SWITCH_REQUEST => {
            response.tag = account_tag::SWITCH_REPLY;
            let Some(&target) = request.words.first() else {
                return fail(response, AccountError::InvalidArgument);
            };
            match store.switch_user(target as u32) {
                Ok(SwitchRecord {
                    account_id,
                    from_session,
                    to_session,
                }) => {
                    response.word_count = 4;
                    response.words[0] = 0;
                    response.words[1] = account_id as u64;
                    response.words[2] = from_session as u64;
                    response.words[3] = to_session as u64;
                    false
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == account_tag::POLICY_GET_REQUEST => {
            response.tag = account_tag::POLICY_GET_REPLY;
            let result = match request.words.first().copied().unwrap_or(0) {
                0 => store.active_capabilities(),
                account_id => store.capabilities_of(account_id as u32),
            };
            match result {
                Ok(caps) => {
                    response.word_count = 2;
                    response.words[0] = 0;
                    response.words[1] = caps as u64;
                    false
                }
                Err(error) => fail(response, error),
            }
        }
        x if x == account_tag::LIST_REQUEST => {
            response.tag = account_tag::LIST_REPLY;
            response.word_count = (2 + store.count * 2) as u32;
            response.words[0] = 0;
            response.words[1] = store.count as u64;
            for index in 0..store.count {
                let account = &store.accounts[index];
                response.words[2 + index * 2] = account.id as u64;
                response.words[3 + index * 2] = account.capabilities as u64;
            }
            false
        }
        _ => false,
    }
}

/// Stamp an error reply; returns false so `return fail(..)` short-circuits
/// handle_request with a clean "not dirty" verdict.
fn fail(response: &mut RawMessage, error: AccountError) -> bool {
    response.word_count = 1;
    response.words[0] = error.to_code() as u64;
    false
}
