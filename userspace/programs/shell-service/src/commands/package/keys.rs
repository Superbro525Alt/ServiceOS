use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

use super::mutate::{package_status_name, status_from_word};

/// Mirrors package-service keystore wire limits (signing.rs); the shell
/// speaks the keystore protocol directly so it needs the field caps only.
const SOURCE_NAME_MAX: usize = 32;
const KEY_ID_MAX: usize = 24;
const KEY_HEX_LEN: usize = 64;

const ALG_WORD_FNV: u64 = 1;
const ALG_WORD_ED25519: u64 = 2;
const STATE_WORD_ACTIVE: u64 = 1;
const STATE_WORD_RETIRED: u64 = 2;

fn alg_name(word: u64) -> &'static str {
    match word {
        ALG_WORD_FNV => "fnv",
        ALG_WORD_ED25519 => "ed25519",
        _ => "unknown",
    }
}

fn state_name(word: u64) -> &'static str {
    match word {
        STATE_WORD_ACTIVE => "active",
        STATE_WORD_RETIRED => "retired",
        _ => "unknown",
    }
}

const USAGE_KEYS: &str = "usage: pkg keys <list|enroll|activate|rotate|gen>";
const USAGE_ENROLL: &str = "usage: pkg keys enroll <source> <hex-pubkey>";
const USAGE_GEN: &str = "usage: pkg keys gen <source> [--show-seed]";

/// Pack `[len words][inline bytes...]` for every field after the header,
/// setting word_count accordingly. Combined scratch stays on stack.
fn push_packed(
    message: &mut rt::RawMessage,
    header_words: usize,
    fields: [&[u8]; 2],
    field_count: usize,
) -> rt::Result<()> {
    const COMBINED_MAX: usize = SOURCE_NAME_MAX + KEY_HEX_LEN;
    let mut combined = [0u8; COMBINED_MAX];
    let mut cursor = 0usize;
    for field in fields.iter().take(field_count) {
        if cursor + field.len() > COMBINED_MAX {
            return Err(rt::Error::BufferTooSmall);
        }
        combined[cursor..cursor + field.len()].copy_from_slice(field);
        cursor += field.len();
    }
    for (slot, field) in fields.iter().enumerate().take(field_count) {
        message.words[header_words + slot] = field.len() as u64;
    }
    message.word_count = (header_words + field_count) as u32
        + rt::pack_bytes(
            &combined[..cursor],
            &mut message.words[header_words + field_count..],
        )?;
    Ok(())
}

/// Extract a single inline string field from `[len][packed bytes..]`.
fn reply_string<'a>(
    words: &'a [u64],
    word_count: u32,
    len_slot: usize,
    out: &'a mut [u8],
) -> rt::Result<&'a str> {
    let base = len_slot + 1;
    if word_count as usize <= base || out.is_empty() {
        return Err(rt::Error::InvalidArgument);
    }
    let len = words[len_slot] as usize;
    if len == 0 || len > out.len() {
        return Err(rt::Error::InvalidArgument);
    }
    rt::unpack_bytes(&words[base..word_count as usize], len, &mut out[..len])?;
    core::str::from_utf8(&out[..len]).map_err(|_| rt::Error::InvalidArgument)
}

/// Extract `(first, second)` from `[len_a][len_b][packed bytes..]`.
fn reply_two_strings<'a>(
    words: &'a [u64],
    word_count: u32,
    len_slot: usize,
    data_base: usize,
    out: &'a mut [u8],
) -> rt::Result<(&'a str, &'a str)> {
    let base = data_base;
    if word_count as usize <= base {
        return Err(rt::Error::InvalidArgument);
    }
    let len_a = words[len_slot] as usize;
    let len_b = words[len_slot + 1] as usize;
    if len_a
        .checked_add(len_b)
        .map(|total| total == 0 || total > out.len())
        != Some(false)
    {
        return Err(rt::Error::InvalidArgument);
    }
    rt::unpack_bytes(
        &words[base..word_count as usize],
        len_a + len_b,
        &mut out[..],
    )?;
    let first = core::str::from_utf8(&out[..len_a]).map_err(|_| rt::Error::InvalidArgument)?;
    let second =
        core::str::from_utf8(&out[len_a..len_a + len_b]).map_err(|_| rt::Error::InvalidArgument)?;
    Ok((first, second))
}

struct KeysHandle {
    package_handle: rt::Handle,
}

impl KeysHandle {
    fn open(bootstrap: rt::Handle) -> rt::Result<Self> {
        Ok(Self {
            package_handle: rt::lookup_service(bootstrap, rt::ServiceId::Package)?,
        })
    }

    /// One keystore request/reply round trip; validates the reply tag.
    fn call(
        &self,
        tag: rt::PackageTag,
        fill: impl FnOnce(&mut rt::RawMessage),
    ) -> rt::Result<rt::RawMessage> {
        let mut request = rt::RawMessage::empty(tag as u32);
        fill(&mut request);
        let response = rt::channel_call(self.package_handle, &mut request)?;
        let expected = match tag {
            rt::PackageTag::KeysListRequest => rt::PackageTag::KeysListReply,
            rt::PackageTag::KeysEnrollRequest => rt::PackageTag::KeysEnrollReply,
            rt::PackageTag::KeysActivateRequest => rt::PackageTag::KeysActivateReply,
            rt::PackageTag::KeysRotateRequest => rt::PackageTag::KeysRotateReply,
            rt::PackageTag::KeysGenRequest => rt::PackageTag::KeysGenReply,
            _ => return Err(rt::Error::InvalidArgument),
        };
        if response.tag != expected as u32 {
            return Err(rt::Error::InvalidArgument);
        }
        Ok(response)
    }
}

impl Drop for KeysHandle {
    fn drop(&mut self) {
        let _ = rt::handle_close(self.package_handle);
    }
}

pub(in crate::commands) fn cmd_pkg_keys<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("list") => cmd_keys_list(bootstrap, output),
        Some("enroll") => match (parts.next(), parts.next(), parts.next()) {
            (Some(source), Some(hex), None) => cmd_keys_enroll(bootstrap, output, source, hex),
            _ => write_output_linef(output, format_args!("{USAGE_ENROLL}")),
        },
        Some("activate") => match (parts.next(), parts.next()) {
            (Some(key_id), None) => cmd_keys_activate(bootstrap, output, key_id),
            _ => write_output_linef(output, format_args!("usage: pkg keys activate <id>")),
        },
        Some("rotate") => match (parts.next(), parts.next()) {
            (Some(source), None) => cmd_keys_rotate(bootstrap, output, source),
            _ => write_output_linef(output, format_args!("usage: pkg keys rotate <source>")),
        },
        Some("gen") => cmd_keys_gen_with_flags(bootstrap, output, parts.collect::<Args>()),
        _ => write_output_linef(output, format_args!("{USAGE_KEYS}")),
    }
}

#[derive(Default)]
struct Args {
    positional: heapless_string::HeapString,
    show_seed: bool,
}

impl<'a> FromIterator<&'a str> for Args {
    fn from_iter<T: IntoIterator<Item = &'a str>>(iter: T) -> Self {
        let mut args = Args::default();
        for token in iter {
            if token == "--show-seed" {
                args.show_seed = true;
            } else {
                args.positional.set(token);
            }
        }
        args
    }
}

mod heapless_string {
    /// Fixed-capacity positional argument carrier (no alloc on host or guest).
    pub(crate) struct HeapString {
        bytes: [u8; 96],
        len: usize,
    }

    impl Default for HeapString {
        fn default() -> Self {
            Self {
                bytes: [0; 96],
                len: 0,
            }
        }
    }

    impl HeapString {
        pub(crate) fn set(&mut self, value: &str) {
            self.bytes = [0; 96];
            self.len = value.len().min(self.bytes.len());
            self.bytes[..self.len].copy_from_slice(&value.as_bytes()[..self.len]);
        }

        pub(crate) fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }
}

fn cmd_keys_gen_with_flags(
    bootstrap: rt::Handle,
    output: ShellOutput,
    args: Args,
) -> rt::Result<()> {
    let source = args.positional.as_str();
    if source.is_empty() {
        write_output_linef(output, format_args!("{USAGE_GEN}"))
    } else {
        cmd_keys_gen(bootstrap, output, source, args.show_seed)
    }
}

/// `pkg keys list` — flattened keystore rows with algorithm/keyid/state/
/// active and retired-tick columns.
fn cmd_keys_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let keys = KeysHandle::open(bootstrap)?;
    let mut index = 0u64;
    loop {
        let reply = keys.call(rt::PackageTag::KeysListRequest, |request| {
            request.word_count = 1;
            request.words[0] = index;
        })?;
        if reply.word_count < 8 {
            return Err(rt::Error::InvalidArgument);
        }
        match status_of(&reply) {
            StatusWord::End => break,
            StatusWord::Ok => {}
            StatusWord::Fail(word) => {
                write_output_linef(output, format_args!("keys list failed: {}", word))?;
                return Ok(());
            }
        }
        let alg = reply.words[2];
        let state = reply.words[3];
        let retired_tick = reply.words[4];
        let mut names = [0u8; SOURCE_NAME_MAX + KEY_ID_MAX];
        let (source_text, id_text) =
            reply_two_strings(&reply.words, reply.word_count, 5, 8, &mut names)?;
        write_output_linef(
            output,
            format_args!(
                "#{index} src={source} id={id} alg={alg_name} state={state_name} active={active} retired-tick={tick}",
                source = source_text,
                id = id_text,
                alg_name = alg_name(alg),
                state_name = state_name(state),
                active = if state == STATE_WORD_ACTIVE {
                    "yes"
                } else {
                    "no"
                },
                tick = retired_tick,
            ),
        )?;
        index += 1;
    }
    if index == 0 {
        write_output_linef(output, format_args!("no feed signing keys"))
    } else {
        Ok(())
    }
}

enum StatusWord {
    Ok,
    End,
    Fail(&'static str),
}

fn status_of(reply: &rt::RawMessage) -> StatusWord {
    let status = status_from_word(reply.words[0]);
    if matches!(status, rt::PackageStatus::Ok) {
        StatusWord::Ok
    } else if matches!(status, rt::PackageStatus::End) {
        StatusWord::End
    } else {
        StatusWord::Fail(package_status_name(status))
    }
}

/// `pkg keys enroll <source> <hex>` — enroll one Ed25519 verification key.
fn cmd_keys_enroll(
    bootstrap: rt::Handle,
    output: ShellOutput,
    source: &str,
    hex_pub: &str,
) -> rt::Result<()> {
    if source.is_empty() || source.len() > SOURCE_NAME_MAX || !is_hex_64(hex_pub) {
        write_output_linef(
            output,
            format_args!("{USAGE_ENROLL} (hex must be 64 hex chars = ed25519 pubkey)"),
        )
    } else {
        let keys = KeysHandle::open(bootstrap)?;
        let lower_owned = to_lower(hex_pub);
        let reply = keys.call(rt::PackageTag::KeysEnrollRequest, |request| {
            request.words[0] = 0; // reserved algorithm slot (ed25519 implied)
            let _ = push_packed(request, 1, [source.as_bytes(), lower_owned.as_bytes()], 2);
        })?;
        if reply.word_count < 2 {
            return Err(rt::Error::InvalidArgument);
        }
        match status_of(&reply) {
            StatusWord::Ok => {
                let state = reply.words[1];
                write_output_linef(
                    output,
                    format_args!(
                        "enrolled id={} alg=ed25519 state={} active={}",
                        derive_shell_id_preview(hex_pub).as_str(),
                        state_name(state),
                        if state == STATE_WORD_ACTIVE {
                            "yes"
                        } else {
                            "no"
                        },
                    ),
                )
            }
            StatusWord::End => Err(rt::Error::InvalidArgument),
            StatusWord::Fail(name) => {
                write_output_linef(output, format_args!("keys enroll failed: {}", name))
            }
        }
    }
}

/// Shell-side mirror of the service's material-derived auto key id so the
/// enroll confirmation can print the id immediately (`k-<fnv16 hex>`).
fn derive_shell_id_preview(key_hex: &str) -> HeaplessId {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x100_0000_01b3;
    let mut word = OFFSET;
    for byte in key_hex.bytes() {
        word ^= u64::from(byte);
        word = word.wrapping_mul(PRIME);
    }
    let mut id = HeaplessId::new();
    let prefix = b"k-";
    id.bytes[..2].copy_from_slice(prefix);
    for index in 0..16 {
        let nibble = ((word >> (60 - 4 * index)) & 0xf) as u8;
        id.bytes[2 + index] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
    }
    id.len = 18;
    id
}

struct HeaplessId {
    bytes: [u8; KEY_ID_MAX + 4],
    len: usize,
}

impl HeaplessId {
    fn new() -> Self {
        Self {
            bytes: [0; KEY_ID_MAX + 4],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

fn is_hex_64(text: &str) -> bool {
    text.len() == KEY_HEX_LEN
        && text.bytes().all(|byte| {
            byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || (b'A'..=b'F').contains(&byte)
        })
}

fn to_lower(text: &str) -> HeaplessHex {
    let mut out = HeaplessHex::new();
    for byte in text.bytes().take(KEY_HEX_LEN) {
        out.bytes[out.len] = byte.to_ascii_lowercase();
        out.len += 1;
    }
    out
}

struct HeaplessHex {
    bytes: [u8; KEY_HEX_LEN],
    len: usize,
}

impl HeaplessHex {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl HeaplessHex {
    fn new() -> Self {
        Self {
            bytes: [0; KEY_HEX_LEN],
            len: 0,
        }
    }
}

/// Resolve which source pins `key_id` by scanning the flat listing.
fn resolve_id_source(keys: &KeysHandle, key_id: &str) -> rt::Result<Option<SourceMatch>> {
    let mut index = 0u64;
    loop {
        let reply = keys.call(rt::PackageTag::KeysListRequest, |request| {
            request.word_count = 1;
            request.words[0] = index;
        })?;
        if reply.word_count < 8 {
            return Err(rt::Error::InvalidArgument);
        }
        match status_of(&reply) {
            StatusWord::End => return Ok(None),
            StatusWord::Ok => {}
            StatusWord::Fail(_) => return Err(rt::Error::InvalidArgument),
        }
        let mut names = [0u8; SOURCE_NAME_MAX + KEY_ID_MAX];
        let (source_text, id_text) =
            reply_two_strings(&reply.words, reply.word_count, 5, 8, &mut names)?;
        if id_text == key_id {
            let mut source_owned = SourceMatch::empty();
            source_owned.set(source_text)?;
            return Ok(Some(source_owned));
        }
        index += 1;
    }
}

struct SourceMatch {
    bytes: [u8; SOURCE_NAME_MAX],
    len: usize,
}

impl SourceMatch {
    fn empty() -> Self {
        Self {
            bytes: [0; SOURCE_NAME_MAX],
            len: 0,
        }
    }

    fn set(&mut self, value: &str) -> rt::Result<()> {
        if value.is_empty() || value.len() > SOURCE_NAME_MAX {
            return Err(rt::Error::InvalidArgument);
        }
        self.bytes[..value.len()].copy_from_slice(value.as_bytes());
        self.len = value.len();
        Ok(())
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

/// `pkg keys activate <id>` — promote an enrolled key of any source to
/// active; the owning source is resolved from the live listing.
fn cmd_keys_activate(bootstrap: rt::Handle, output: ShellOutput, key_id: &str) -> rt::Result<()> {
    let keys = KeysHandle::open(bootstrap)?;
    let Some(matched) = resolve_id_source(&keys, key_id)? else {
        return write_output_linef(output, format_args!("keys activate failed: unknown key"));
    };
    let now = rt::monotonic_now().unwrap_or(0);
    let reply = keys.call(rt::PackageTag::KeysActivateRequest, |request| {
        request.words[0] = now;
        let _ = push_packed(
            request,
            1,
            [matched.as_str().as_bytes(), key_id.as_bytes()],
            2,
        );
    })?;
    if reply.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }
    match status_of(&reply) {
        StatusWord::Ok => write_output_linef(
            output,
            format_args!(
                "activated id={} src={} retired-old=yes",
                key_id,
                matched.as_str()
            ),
        ),
        StatusWord::End => Err(rt::Error::InvalidArgument),
        StatusWord::Fail(name) => {
            write_output_linef(output, format_args!("keys activate failed: {}", name))
        }
    }
}

/// `pkg keys rotate <source>` — promote the newest enrolled standby.
fn cmd_keys_rotate(bootstrap: rt::Handle, output: ShellOutput, source: &str) -> rt::Result<()> {
    let keys = KeysHandle::open(bootstrap)?;
    let now = rt::monotonic_now().unwrap_or(0);
    let reply = keys.call(rt::PackageTag::KeysRotateRequest, |request| {
        request.words[0] = now;
        let _ = push_packed(request, 1, [source.as_bytes(), b""], 1);
    })?;
    if reply.word_count < 2 {
        return Err(rt::Error::InvalidArgument);
    }
    match status_of(&reply) {
        StatusWord::Ok => {
            let mut id_out = [0u8; KEY_ID_MAX];
            let promoted = reply_string(&reply.words, reply.word_count, 1, &mut id_out)?;
            write_output_linef(
                output,
                format_args!(
                    "rotated src={} active-id={} old-key=retired",
                    source, promoted
                ),
            )
        }
        StatusWord::End => Err(rt::Error::InvalidArgument),
        StatusWord::Fail(name) => {
            write_output_linef(output, format_args!("keys rotate failed: {}", name))
        }
    }
}

/// `pkg keys gen <source> [--show-seed]` — generate a fresh Ed25519 pair
/// inside the package service. Default reply carries the PUBLIC key; with
/// --show-seed the once-only SECRET seed replaces it in the output.
fn cmd_keys_gen(
    bootstrap: rt::Handle,
    output: ShellOutput,
    source: &str,
    show_seed: bool,
) -> rt::Result<()> {
    let keys = KeysHandle::open(bootstrap)?;
    let reply = keys.call(rt::PackageTag::KeysGenRequest, |request| {
        request.words[0] = u64::from(show_seed);
        let _ = push_packed(request, 1, [source.as_bytes(), b""], 1);
    })?;
    if reply.word_count < 4 {
        return Err(rt::Error::InvalidArgument);
    }
    match status_of(&reply) {
        StatusWord::Ok => {
            let state = reply.words[1];
            let mut pair_out = [0u8; KEY_ID_MAX + KEY_HEX_LEN];
            let (id_text, secret_text) =
                reply_two_strings(&reply.words, reply.word_count, 2, 4, &mut pair_out)?;
            if show_seed {
                write_output_linef(
                    output,
                    format_args!(
                        "generated src={} id={} alg=ed25519 state={}",
                        source,
                        id_text,
                        state_name(state)
                    ),
                )?;
                write_output_linef(output, format_args!("public=enrolled; see 'pkg keys list'"))?;
                write_output_linef(
                    output,
                    format_args!("secret-seed={} (shown ONCE; never stored)", secret_text),
                )?;
                write_output_linef(
                    output,
                    format_args!(
                        "caveat: no kernel RNG yet - guest seeds are boot-local substitutes; prefer host-generated keys for production"
                    ),
                )
            } else {
                write_output_linef(
                    output,
                    format_args!(
                        "generated src={} id={} alg=ed25519 state={} public={}",
                        source,
                        id_text,
                        state_name(state),
                        secret_text
                    ),
                )?;
                write_output_linef(
                    output,
                    format_args!(
                        "note: rerun with --show-seed to receive the secret seed once (never stored)"
                    ),
                )
            }
        }
        StatusWord::End => Err(rt::Error::InvalidArgument),
        StatusWord::Fail(name) => {
            write_output_linef(output, format_args!("keys gen failed: {}", name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alg_and_state_names_cover_wire_words() {
        assert_eq!(alg_name(ALG_WORD_FNV), "fnv");
        assert_eq!(alg_name(ALG_WORD_ED25519), "ed25519");
        assert_eq!(alg_name(9), "unknown");
        assert_eq!(state_name(STATE_WORD_ACTIVE), "active");
        assert_eq!(state_name(STATE_WORD_RETIRED), "retired");
    }

    #[test]
    fn is_hex_64_validates_length_and_charset() {
        assert!(is_hex_64(&"ab".repeat(32)));
        assert!(is_hex_64(&"AB".repeat(32)));
        assert!(!is_hex_64(&"ab".repeat(31)));
        assert!(!is_hex_64(&"zz".repeat(32)));
    }

    #[test]
    fn id_preview_matches_service_format() {
        let left = derive_shell_id_preview(&"aa".repeat(32));
        let right = derive_shell_id_preview(&"aa".repeat(32));
        assert_eq!(left.as_str(), right.as_str());
        assert!(left.as_str().starts_with("k-"));
        assert_eq!(left.as_str().len(), 18);
        let other = derive_shell_id_preview(&"bb".repeat(32));
        assert_ne!(left.as_str(), other.as_str());
    }

    #[test]
    fn to_lower_normalizes_case_preserving_length() {
        let lowered = to_lower(&"AbCd".repeat(16));
        assert_eq!(lowered.as_bytes(), b"abcd".repeat(16).as_slice());
        assert_eq!(lowered.len, KEY_HEX_LEN);
    }

    #[test]
    fn push_packed_sets_lengths_and_word_count() {
        let mut message = rt::RawMessage::empty(0x720);
        message.words[0] = 5;
        push_packed(&mut message, 1, [b"boot", b"c0ffee00"], 2).unwrap();
        assert_eq!(message.words[1], 4);
        assert_eq!(message.words[2], 8);
        assert_eq!(message.word_count, 3 + 2);
    }

    #[test]
    fn reply_two_strings_roundtrip_through_push_packed() {
        let mut message = rt::RawMessage::empty(0x721);
        message.words[0] = 0;
        push_packed(&mut message, 1, [b"src", b"ident"], 2).unwrap();
        // Response layout mirrors requests: lens occupy slots 0..2 here.
        let mut buffer = [0u8; SOURCE_NAME_MAX + KEY_ID_MAX];
        let (left, right) =
            reply_two_strings(&message.words, message.word_count, 1, 1 + 2, &mut buffer).unwrap();
        assert_eq!(left, "src");
        assert_eq!(right, "ident");
    }
}
