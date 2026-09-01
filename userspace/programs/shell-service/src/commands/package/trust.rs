//! `pkg trust` — operator-managed trust-root list for feed-signing keys.
//! Speaks the trust-root protocol (PackageTag 0x732..) directly against
//! package-service, mirroring the keystore/rollout command blocks.
//!
//! Standing model (v0, management layer only — no crypto chaining):
//! ROOT = key id on the operator-managed root list; DIRECT = enrolled while
//! a root regime existed (keystore record carries enrolled-at + via);
//! UNATTESTED = legacy pre-root record, displayed honestly as such.

use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

use super::mutate::{package_status_name, status_from_word};

/// Mirrors package-service trust-root wire limits (signing.rs).
const KEY_ID_MAX: usize = 24;
const ROOT_LABEL_MAX: usize = 24;

const USAGE_TRUST: &str = "usage: pkg trust <list|add|remove>";
const USAGE_ADD: &str = "usage: pkg trust add <key-id> [label] --yes";
const USAGE_REMOVE: &str = "usage: pkg trust remove <key-id> --yes";

struct TrustHandle {
    package_handle: rt::Handle,
}

impl TrustHandle {
    fn open(bootstrap: rt::Handle) -> rt::Result<Self> {
        Ok(Self {
            package_handle: rt::lookup_service(bootstrap, rt::ServiceId::Package)?,
        })
    }

    /// One trust-root request/reply round trip; validates the reply tag.
    fn call(
        &self,
        tag: rt::PackageTag,
        fill: impl FnOnce(&mut rt::RawMessage),
    ) -> rt::Result<rt::RawMessage> {
        let mut request = rt::RawMessage::empty(tag as u32);
        fill(&mut request);
        let response = rt::channel_call(self.package_handle, &mut request)?;
        let expected = match tag {
            rt::PackageTag::RootListRequest => rt::PackageTag::RootListReply,
            rt::PackageTag::RootGetRequest => rt::PackageTag::RootGetReply,
            rt::PackageTag::RootAddRequest => rt::PackageTag::RootAddReply,
            rt::PackageTag::RootRemoveRequest => rt::PackageTag::RootRemoveReply,
            _ => return Err(rt::Error::InvalidArgument),
        };
        if response.tag != expected as u32 {
            return Err(rt::Error::InvalidArgument);
        }
        Ok(response)
    }
}

impl Drop for TrustHandle {
    fn drop(&mut self) {
        let _ = rt::handle_close(self.package_handle);
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

/// Pack `[len words][inline bytes...]` after the header, setting word_count.
/// Both fields are length-capped by the parser before this runs.
fn push_packed(
    message: &mut rt::RawMessage,
    header_words: usize,
    fields: [&[u8]; 2],
) -> rt::Result<()> {
    const COMBINED_MAX: usize = KEY_ID_MAX + ROOT_LABEL_MAX;
    let mut combined = [0u8; COMBINED_MAX];
    let mut cursor = 0usize;
    for field in fields {
        if cursor + field.len() > COMBINED_MAX {
            return Err(rt::Error::BufferTooSmall);
        }
        combined[cursor..cursor + field.len()].copy_from_slice(field);
        cursor += field.len();
    }
    for (slot, field) in fields.iter().enumerate() {
        message.words[header_words + slot] = field.len() as u64;
    }
    message.word_count = (header_words + fields.len()) as u32
        + rt::pack_bytes(
            &combined[..cursor],
            &mut message.words[header_words + fields.len()..],
        )? as u32;
    Ok(())
}

/// Extract `(first, second)` from `[len_a][len_b][packed bytes..]` at
/// `len_slot`, data beginning at `data_base`.
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

pub(in crate::commands) fn cmd_pkg_trust<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("list") => cmd_trust_list(bootstrap, output),
        Some("add") => match TrustAddArgs::parse(parts) {
            Some(args) => cmd_trust_add(bootstrap, output, args.key_id(), args.label()),
            None => write_output_linef(output, format_args!("{USAGE_ADD}")),
        },
        Some("remove") => match (parts.next(), parts.next(), parts.next()) {
            (Some(key_id), Some("--yes"), None) => cmd_trust_remove(bootstrap, output, key_id),
            _ => write_output_linef(output, format_args!("{USAGE_REMOVE}")),
        },
        _ => write_output_linef(output, format_args!("{USAGE_TRUST}")),
    }
}

/// Stack-resident `pkg trust add` arguments (no alloc on host or guest).
struct TrustAddArgs {
    key_id: [u8; KEY_ID_MAX],
    key_id_len: usize,
    label: [u8; ROOT_LABEL_MAX],
    label_len: usize,
    confirmed: bool,
}

impl TrustAddArgs {
    fn parse<'a, I>(parts: I) -> Option<Self>
    where
        I: Iterator<Item = &'a str>,
    {
        let mut args = Self {
            key_id: [0; KEY_ID_MAX],
            key_id_len: 0,
            label: [0; ROOT_LABEL_MAX],
            label_len: 0,
            confirmed: false,
        };
        let mut tokens = parts.into_iter();
        let key_id = tokens.next()?;
        for token in tokens {
            if token == "--yes" {
                args.confirmed = true;
            } else if args.label_len == 0 && !token.is_empty() && token.len() <= ROOT_LABEL_MAX {
                args.label[..token.len()].copy_from_slice(token.as_bytes());
                args.label_len = token.len();
            }
        }
        if key_id.is_empty() || key_id.len() > KEY_ID_MAX {
            return None;
        }
        args.key_id[..key_id.len()].copy_from_slice(key_id.as_bytes());
        args.key_id_len = key_id.len();
        if !args.confirmed {
            return None;
        }
        Some(args)
    }

    fn key_id(&self) -> &str {
        core::str::from_utf8(&self.key_id[..self.key_id_len]).unwrap_or("")
    }

    fn label(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len]).unwrap_or("")
    }
}

/// `pkg trust list` — root rows with label, enrolled tick, derived count.
fn cmd_trust_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let trust = TrustHandle::open(bootstrap)?;
    let mut index = 0u64;
    loop {
        let reply = trust.call(rt::PackageTag::RootListRequest, |request| {
            request.word_count = 1;
            request.words[0] = index;
        })?;
        if reply.word_count < 1 {
            return Err(rt::Error::InvalidArgument);
        }
        match status_of(&reply) {
            StatusWord::End => break,
            StatusWord::Ok => {}
            StatusWord::Fail(name) => {
                write_output_linef(output, format_args!("trust list failed: {}", name))?;
                return Ok(());
            }
        }
        if reply.word_count < 8 {
            return Err(rt::Error::InvalidArgument);
        }
        let mut names = [0u8; KEY_ID_MAX + ROOT_LABEL_MAX];
        let (id_text, label_text) =
            reply_two_strings(&reply.words, reply.word_count, 2, 8, &mut names)?;
        write_output_linef(
            output,
            format_args!(
                "#{index} id={id} label={label} enrolled-tick={tick} derived={derived}",
                id = id_text,
                label = if label_text.is_empty() {
                    "-"
                } else {
                    label_text
                },
                tick = reply.words[4],
                derived = reply.words[5],
            ),
        )?;
        index += 1;
    }
    if index == 0 {
        write_output_linef(output, format_args!("no trust roots enrolled"))
    } else {
        Ok(())
    }
}

/// `pkg trust add <key-id> [label] --yes` — promote an enrolled key to a
/// trust root. The key must already exist in the keystore.
fn cmd_trust_add(
    bootstrap: rt::Handle,
    output: ShellOutput,
    key_id: &str,
    label: &str,
) -> rt::Result<()> {
    let trust = TrustHandle::open(bootstrap)?;
    let label_text = if label.is_empty() { "root" } else { label };
    let reply = trust.call(rt::PackageTag::RootAddRequest, |request| {
        request.word_count = 1;
        request.words[0] = rt::monotonic_now().unwrap_or(0);
        let _ = push_packed(request, 1, [key_id.as_bytes(), label_text.as_bytes()]);
    })?;
    if reply.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }
    match status_of(&reply) {
        StatusWord::Ok => write_output_linef(
            output,
            format_args!(
                "root added id={} label={} (slot {})",
                key_id,
                if label.is_empty() { "root" } else { label },
                reply.words[1],
            ),
        ),
        StatusWord::End => Err(rt::Error::InvalidArgument),
        StatusWord::Fail(name) => {
            write_output_linef(output, format_args!("trust add failed: {}", name))
        }
    }
}

/// `pkg trust remove <key-id> --yes` — drop a root. Keystore records are
/// untouched; a DIRECT key whose via root is gone simply loses the
/// resolvable reference and displays honestly.
fn cmd_trust_remove(bootstrap: rt::Handle, output: ShellOutput, key_id: &str) -> rt::Result<()> {
    let trust = TrustHandle::open(bootstrap)?;
    let reply = trust.call(rt::PackageTag::RootRemoveRequest, |request| {
        request.word_count = 1;
        request.words[0] = key_id.len() as u64;
        if let Ok(packed) = rt::pack_bytes(key_id.as_bytes(), &mut request.words[1..]) {
            request.word_count += packed as u32;
        }
    })?;
    if reply.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }
    match status_of(&reply) {
        StatusWord::Ok => write_output_linef(output, format_args!("root removed id={}", key_id)),
        StatusWord::End => {
            write_output_linef(output, format_args!("trust remove failed: not-found"))
        }
        StatusWord::Fail(name) => {
            write_output_linef(output, format_args!("trust remove failed: {}", name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_add_args_require_key_id_and_confirmation() {
        let args = TrustAddArgs::parse(["k-abc", "vendor", "--yes"].into_iter()).unwrap();
        assert_eq!(args.key_id(), "k-abc");
        assert_eq!(args.label(), "vendor");

        let bare = TrustAddArgs::parse(["k-abc", "--yes"].into_iter()).unwrap();
        assert_eq!(bare.key_id(), "k-abc");
        assert_eq!(bare.label(), "");

        // --yes is mandatory; key id must exist and fit the wire cap.
        assert!(TrustAddArgs::parse(["k-abc", "vendor"].into_iter()).is_none());
        assert!(TrustAddArgs::parse(["--yes"].into_iter()).is_none());
        assert!(TrustAddArgs::parse(["", "--yes"].into_iter()).is_none());
        assert!(
            TrustAddArgs::parse([core::str::from_utf8(&[b'k'; 25]).unwrap(), "--yes"].into_iter())
                .is_none()
        );
    }

    #[test]
    fn trust_row_roundtrip_through_packed_fields() {
        let mut message = rt::RawMessage::empty(0x733);
        push_packed(&mut message, 6, [b"k-anchor", b"vendor-root"]).unwrap();
        let mut buffer = [0u8; KEY_ID_MAX + ROOT_LABEL_MAX];
        let (id, label) =
            reply_two_strings(&message.words, message.word_count, 6, 8, &mut buffer).unwrap();
        assert_eq!(id, "k-anchor");
        assert_eq!(label, "vendor-root");
    }
}
