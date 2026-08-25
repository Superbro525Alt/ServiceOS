use core::str;

use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_NAME, MAX_PATH},
    types::{FixedBytes, ToolchainSlot},
};

pub(crate) const MAX_PAYLOADS: usize = 4;
/// Writable mirror of the packaged SDK layout. Boot-store content under
/// `packages/` is immutable, so materialized payloads land here instead.
pub(crate) const SDK_STATE_PREFIX: &[u8] = b"state/devsvc/sdk";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PayloadRef {
    /// Blob path in storage read at install time.
    StoragePath(FixedBytes<MAX_PATH>),
    /// Boot-store executable image id; bytes are kernel-authoritative and
    /// not readable from userspace, so install records a reference stub.
    BootStoreImage(u32),
}

#[derive(Clone, Copy)]
pub(crate) struct PayloadSlot {
    pub(crate) name: FixedBytes<MAX_NAME>,
    pub(crate) reference: PayloadRef,
    /// Optional lowercase hex sha256 over the materialized bytes.
    pub(crate) checksum_hex: FixedBytes<64>,
}

impl PayloadSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            name: FixedBytes::empty(),
            reference: PayloadRef::StoragePath(FixedBytes::empty()),
            checksum_hex: FixedBytes::empty(),
        }
    }

    pub(crate) fn wants_checksum(&self) -> bool {
        self.checksum_hex.len > 0
    }
}

/// One planned write of the install operation (host-pure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlanSource {
    CopyFrom(FixedBytes<MAX_PATH>),
    ImageRef(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlanStep {
    pub(crate) dest: FixedBytes<MAX_PATH>,
    pub(crate) source: PlanSource,
    pub(crate) verify_checksum: bool,
}

/// Parse a descriptor `payload=name@ref` value. Ref forms:
/// `image:<decimal boot-store image id>` or `path:<storage path>`.
pub(crate) fn parse_payload_entry(value: &str) -> rt::Result<PayloadSlot> {
    let Some((name, reference)) = value.split_once('@') else {
        return Err(rt::Error::InvalidArgument);
    };
    if name.is_empty() || !name.bytes().all(is_name_byte) {
        return Err(rt::Error::InvalidArgument);
    }
    let reference = parse_payload_ref(reference)?;
    let mut slot = PayloadSlot::empty();
    slot.name.set(name.as_bytes())?;
    slot.reference = reference;
    Ok(slot)
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.'
}

fn parse_payload_ref(value: &str) -> rt::Result<PayloadRef> {
    if let Some(rest) = value.strip_prefix("image:") {
        let id: u32 = rest.parse().map_err(|_| rt::Error::InvalidArgument)?;
        return Ok(PayloadRef::BootStoreImage(id));
    }
    if let Some(rest) = value.strip_prefix("path:") {
        if rest.is_empty() || rest.len() > MAX_PATH {
            return Err(rt::Error::InvalidArgument);
        }
        let mut path = FixedBytes::<MAX_PATH>::empty();
        path.set(rest.as_bytes())?;
        return Ok(PayloadRef::StoragePath(path));
    }
    Err(rt::Error::InvalidArgument)
}

/// Parse a `checksum=<64 hex digits>` value into 32 raw bytes.
pub(crate) fn parse_checksum(value: &str) -> rt::Result<FixedBytes<32>> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(rt::Error::InvalidArgument);
    }
    let mut raw = [0u8; 32];
    for index in 0..32 {
        raw[index] = (hex_nibble(bytes[index * 2])? << 4) | hex_nibble(bytes[index * 2 + 1])?;
    }
    let mut out = FixedBytes::<32>::empty();
    out.set(&raw)?;
    Ok(out)
}

fn hex_nibble(byte: u8) -> rt::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(rt::Error::InvalidArgument),
    }
}

/// Join `prefix` plus path segments with `/` separators into `out`.
/// Returns 0 when the result would overflow (caller skips/flags it).
pub(crate) fn join_path(prefix: &[u8], segments: &[&[u8]], out: &mut [u8]) -> usize {
    let mut cursor = prefix.len();
    if cursor > out.len() {
        return 0;
    }
    out[..cursor].copy_from_slice(prefix);
    for segment in segments {
        if cursor >= out.len() || cursor + 1 + segment.len() > out.len() {
            return 0;
        }
        out[cursor] = b'/';
        cursor += 1;
        out[cursor..cursor + segment.len()].copy_from_slice(segment);
        cursor += segment.len();
    }
    cursor
}

/// Destination path for one payload inside the writable SDK mirror:
/// `state/devsvc/sdk/<toolchain-name>/<name>` plus `.imgref` for image
/// references (they materialize as stubs, not raw bytes). Empty result
/// means the layout would overflow the path budget.
pub(crate) fn payload_dest(
    toolchain_name: &[u8],
    payload: &PayloadSlot,
    dest_buffer: &mut [u8],
) -> usize {
    let suffix: &[u8] = match payload.reference {
        PayloadRef::BootStoreImage(_) => b".imgref",
        PayloadRef::StoragePath(_) => b"",
    };
    let mut full_name = [0u8; MAX_NAME + 8];
    if payload.name.len + suffix.len() > full_name.len() {
        return 0;
    }
    full_name[..payload.name.len].copy_from_slice(payload.name.as_bytes());
    full_name[payload.name.len..payload.name.len + suffix.len()].copy_from_slice(suffix);
    join_path(
        SDK_STATE_PREFIX,
        &[
            toolchain_name,
            &full_name[..payload.name.len + suffix.len()],
        ],
        dest_buffer,
    )
}

/// Build the install plan for a toolchain (pure; host-tested). Returns the
/// number of filled steps; steps beyond the plan capacity or with
/// overflowing destinations are skipped.
pub(crate) fn materialize_plan(toolchain: &ToolchainSlot, steps: &mut [PlanStep]) -> usize {
    let mut count = 0usize;
    for payload in toolchain.payloads[..toolchain.payload_count].iter() {
        if count >= steps.len() {
            break;
        }
        let mut dest = [0u8; MAX_PATH];
        let dest_len = payload_dest(toolchain.name.as_bytes(), payload, &mut dest);
        if dest_len == 0 {
            continue;
        }
        let mut dest_fixed = FixedBytes::<MAX_PATH>::empty();
        if dest_fixed.set(&dest[..dest_len]).is_err() {
            continue;
        }
        let source = match payload.reference {
            PayloadRef::StoragePath(path) => PlanSource::CopyFrom(path),
            PayloadRef::BootStoreImage(id) => PlanSource::ImageRef(id),
        };
        steps[count] = PlanStep {
            dest: dest_fixed,
            source,
            verify_checksum: payload.wants_checksum(),
        };
        count += 1;
    }
    count
}

/// Render a payload reference as `name@ref` text for logs/tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn format_ref<'a>(payload: &PayloadSlot, out: &'a mut [u8]) -> &'a [u8] {
    let mut cursor = 0usize;
    let name = payload.name.as_bytes();
    if cursor + name.len() > out.len() {
        return &[];
    }
    out[cursor..cursor + name.len()].copy_from_slice(name);
    cursor += name.len();
    if cursor >= out.len() {
        return &[];
    }
    out[cursor] = b'@';
    cursor += 1;
    match payload.reference {
        PayloadRef::BootStoreImage(id) => {
            let mut digits = [0u8; 12];
            let mut count = 0usize;
            let mut value = id;
            loop {
                digits[count] = b'0' + (value % 10) as u8;
                count += 1;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            if cursor + 6 + count > out.len() {
                return &[];
            }
            out[cursor..cursor + 6].copy_from_slice(b"image:");
            cursor += 6;
            for byte in digits[..count].iter().rev() {
                out[cursor] = *byte;
                cursor += 1;
            }
        }
        PayloadRef::StoragePath(path) => {
            let bytes = path.as_bytes();
            if cursor + 5 + bytes.len() > out.len() {
                return &[];
            }
            out[cursor..cursor + 5].copy_from_slice(b"path:");
            cursor += 5;
            out[cursor..cursor + bytes.len()].copy_from_slice(bytes);
            cursor += bytes.len();
        }
    }
    &out[..cursor]
}

/// SHA-256 (FIPS 180-4), allocation-free, used for install-time integrity
/// checks on copied payload bytes.
pub(crate) fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (message.len() as u64).wrapping_mul(8);

    let mut block = [0u8; 64];
    let mut offset = 0usize;
    while message.len() - offset >= 64 {
        block.copy_from_slice(&message[offset..offset + 64]);
        compress(&mut state, &block, &K);
        offset += 64;
    }
    let remainder = &message[offset..];
    block[..remainder.len()].copy_from_slice(remainder);
    block[remainder.len()] = 0x80;
    let length_space = 64 - remainder.len() - 1;
    if length_space < 8 {
        for byte in block[remainder.len() + 1..].iter_mut() {
            *byte = 0;
        }
        compress(&mut state, &block, &K);
        block = [0u8; 64];
    } else {
        for byte in block[remainder.len() + 1..64 - 8].iter_mut() {
            *byte = 0;
        }
    }
    block[56..64].copy_from_slice(&bit_len.to_be_bytes());
    compress(&mut state, &block, &K);

    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn compress(state: &mut [u32; 8], block: &[u8; 64], k: &[u32; 64]) {
    let mut w = [0u32; 64];
    for (index, word) in block.chunks(4).enumerate() {
        w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for index in 16..64 {
        let s0 =
            w[index - 15].rotate_right(7) ^ w[index - 15].rotate_right(18) ^ (w[index - 15] >> 3);
        let s1 =
            w[index - 2].rotate_right(17) ^ w[index - 2].rotate_right(19) ^ (w[index - 2] >> 10);
        w[index] = w[index - 16]
            .wrapping_add(s0)
            .wrapping_add(w[index - 7])
            .wrapping_add(s1);
    }
    let mut v = *state;
    for index in 0..64 {
        let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
        let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
        let temp1 = v[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[index])
            .wrapping_add(w[index]);
        let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
        let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let temp2 = s0.wrapping_add(maj);
        v[7] = v[6];
        v[6] = v[5];
        v[5] = v[4];
        v[4] = v[3].wrapping_add(temp1);
        v[3] = v[2];
        v[2] = v[1];
        v[1] = v[0];
        v[0] = temp1.wrapping_add(temp2);
    }
    for index in 0..8 {
        state[index] = state[index].wrapping_add(v[index]);
    }
}

/// Verify materialized bytes against the declared checksum; an empty
/// declaration means "not verified" and always passes.
pub(crate) fn checksum_matches(payload: &PayloadSlot, bytes: &[u8]) -> bool {
    if !payload.wants_checksum() {
        return true;
    }
    let digest = sha256(bytes);
    let declared = payload.checksum_hex.as_bytes();
    declared.len() == 64
        && digest.iter().enumerate().all(|(index, word)| {
            let high = hex_digit_lower((word >> 4) & 0xF);
            let low = hex_digit_lower(word & 0xF);
            declared[index * 2] == high && declared[index * 2 + 1] == low
        })
}

fn hex_digit_lower(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + nibble - 10
    }
}

/// Install-time half: copy planned payloads through storage. Returns
/// `(installed, failed)` counts; checksum mismatch or unreadable sources
/// count as failures and leave that destination unwritten.
pub(crate) const PAYLOAD_COPY_MAX: usize = 512;

pub(crate) fn install_toolchain_payloads(
    storage_handle: rt::Handle,
    toolchain: &ToolchainSlot,
) -> (usize, usize) {
    let mut steps = [PlanStep {
        dest: FixedBytes::empty(),
        source: PlanSource::CopyFrom(FixedBytes::empty()),
        verify_checksum: false,
    }; MAX_PAYLOADS];
    let count = materialize_plan(toolchain, &mut steps);
    let mut installed = 0usize;
    let mut failed = 0usize;
    for step in steps[..count].iter() {
        match write_step(storage_handle, toolchain, step) {
            true => installed += 1,
            false => failed += 1,
        }
    }
    (installed, failed)
}

fn write_step(storage_handle: rt::Handle, toolchain: &ToolchainSlot, step: &PlanStep) -> bool {
    let Ok(dest_path) = str::from_utf8(step.dest.as_bytes()) else {
        return false;
    };
    match step.source {
        PlanSource::ImageRef(id) => {
            let mut stub = [0u8; 32];
            write_blob(storage_handle, dest_path, format_image_stub(id, &mut stub))
        }
        PlanSource::CopyFrom(source_path) => {
            let Ok(source_str) = str::from_utf8(source_path.as_bytes()) else {
                return false;
            };
            let Ok((blob, size)) = rt::storage_open(storage_handle, source_str) else {
                return false;
            };
            let mut buffer = [0u8; PAYLOAD_COPY_MAX];
            let read = match rt::storage_read_all(blob, &mut buffer, size.min(PAYLOAD_COPY_MAX)) {
                Ok(read) => read,
                Err(_) => {
                    let _ = rt::storage_blob_close(blob);
                    return false;
                }
            };
            let _ = rt::storage_blob_close(blob);
            if step.verify_checksum {
                match matching_payload(toolchain, step.dest.as_bytes()) {
                    Some(slot) if !checksum_matches(&slot, &buffer[..read]) => return false,
                    _ => {}
                }
            }
            write_blob(storage_handle, dest_path, &buffer[..read])
        }
    }
}

fn matching_payload(toolchain: &ToolchainSlot, dest: &[u8]) -> Option<PayloadSlot> {
    toolchain.payloads[..toolchain.payload_count]
        .iter()
        .find(|payload| {
            let mut buffer = [0u8; MAX_PATH];
            let len = payload_dest(toolchain.name.as_bytes(), payload, &mut buffer);
            len == dest.len() && &buffer[..len] == dest
        })
        .copied()
}

fn format_image_stub<'a>(id: u32, out: &'a mut [u8]) -> &'a [u8] {
    const PREFIX: &[u8] = b"image=";
    out[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut cursor = PREFIX.len();
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    let mut value = id;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for byte in digits[..count].iter().rev() {
        out[cursor] = *byte;
        cursor += 1;
    }
    out[cursor] = b'\n';
    cursor += 1;
    &out[..cursor]
}

/// Create the `state/devsvc/sdk/<toolchain>/` chain (best effort) and write
/// `bytes` to `file_name` inside it. Existing directories make the create
/// call fail harmlessly; traversal re-opens each level by absolute path.
fn write_blob(storage_handle: rt::Handle, path: &str, bytes: &[u8]) -> bool {
    let segments: [&str; 5] = {
        let mut parts = [""; 5];
        let mut count = 0usize;
        for part in path.split('/') {
            if count >= 5 {
                return false;
            }
            parts[count] = part;
            count += 1;
        }
        // Expect state/devsvc/sdk/<toolchain>/<file>
        if count != 5 || parts[4].is_empty() {
            return false;
        }
        parts
    };

    let mut dir = match rt::storage_open_directory(storage_handle, "state/", true) {
        Ok(handle) => handle,
        Err(_) => return false,
    };
    for level in ["devsvc", "sdk", segments[3]] {
        let _ = rt::storage_directory_create(dir, level, rt::StorageEntryKind::Directory);
        let next = match rt::storage_directory_open_path(dir, level, true) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = rt::handle_close(dir);
                return false;
            }
        };
        let _ = rt::handle_close(dir);
        dir = next;
    }
    let opened = rt::storage_directory_open_file(dir, segments[4], true, true);
    let _ = rt::handle_close(dir);
    match opened {
        Ok((file, _)) => {
            let written = rt::storage_write(file, 0, bytes.len(), bytes).is_ok();
            let _ = rt::storage_blob_close(file);
            written
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_with_payload(name: &[u8], value: &str, checksum: Option<&str>) -> ToolchainSlot {
        let mut toolchain = ToolchainSlot::empty();
        let _ = toolchain.name.set(name);
        let entry = parse_payload_entry(value).expect("payload entry parses");
        toolchain.payloads[0] = entry;
        if let Some(hex) = checksum {
            toolchain.payloads[0]
                .checksum_hex
                .set(hex.as_bytes())
                .expect("hex fits");
        }
        toolchain.payload_count = 1;
        toolchain
    }

    #[test]
    fn payload_entry_parses_name_and_refs() {
        let entry = parse_payload_entry("builder@image:24").unwrap();
        assert_eq!(entry.name.as_bytes(), b"builder");
        assert_eq!(entry.reference, PayloadRef::BootStoreImage(24));
        assert!(!entry.wants_checksum());

        let entry = parse_payload_entry("meta@path:packages/x/README.txt").unwrap();
        let mut expected = FixedBytes::<MAX_PATH>::empty();
        expected.set(b"packages/x/README.txt").unwrap();
        assert_eq!(entry.reference, PayloadRef::StoragePath(expected));
    }

    #[test]
    fn payload_entry_rejects_garbage() {
        for bad in [
            "noatsign",
            "@image:1",
            "bad name@image:1",
            "name@blob:1",
            "name@image:",
            "name@image:xx",
            "name@path:",
        ] {
            assert!(parse_payload_entry(bad).is_err(), "expected reject {bad}");
        }
    }

    #[test]
    fn checksum_hex_parses_to_bytes() {
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let parsed = parse_checksum(hex).unwrap();
        assert_eq!(parsed.as_bytes()[0], 0xba);
        assert_eq!(parsed.as_bytes()[31], 0xad);
        assert!(parse_checksum("xyz").is_err());
        assert!(parse_checksum(&"g".repeat(64)).is_err());
    }

    #[test]
    fn sha256_known_vectors() {
        let empty = sha256(b"");
        assert_eq!(
            str::from_utf8(&format_digest(&empty)).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let abc = sha256(b"abc");
        assert_eq!(
            str::from_utf8(&format_digest(&abc)).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 56-byte input lands exactly on the padding boundary; 112-byte
        // input exercises multiple compress blocks.
        let edge = sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            str::from_utf8(&format_digest(&edge)).unwrap(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let long = sha256(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu");
        assert_eq!(
            str::from_utf8(&format_digest(&long)).unwrap(),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
        );
    }

    fn format_digest(digest: &[u8; 32]) -> [u8; 64] {
        let mut out = [0u8; 64];
        for (index, byte) in digest.iter().enumerate() {
            let high = hex_digit_lower(byte >> 4);
            let low = hex_digit_lower(byte & 0xF);
            out[index * 2] = high;
            out[index * 2 + 1] = low;
        }
        out
    }

    #[test]
    fn checksum_match_decision() {
        let abc_hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let mut payload = parse_payload_entry("m@path:x").unwrap();
        payload.checksum_hex.set(abc_hex.as_bytes()).unwrap();
        assert!(checksum_matches(&payload, b"abc"));
        assert!(!checksum_matches(&payload, b"abcd"));
        let unverified = parse_payload_entry("m@path:x").unwrap();
        assert!(checksum_matches(&unverified, b"anything"));
    }

    #[test]
    fn materialize_plan_lays_out_mirror_paths() {
        let mut toolchain = slot_with_payload(
            b"linux-x64",
            "metadata@path:packages/d/sdk/linux/README.txt",
            None,
        );
        toolchain.payloads[1] = parse_payload_entry("builder@image:24").unwrap();
        toolchain.payload_count = 2;
        let mut steps = [PlanStep {
            dest: FixedBytes::empty(),
            source: PlanSource::CopyFrom(FixedBytes::empty()),
            verify_checksum: false,
        }; MAX_PAYLOADS];
        let count = materialize_plan(&toolchain, &mut steps);
        assert_eq!(count, 2);
        assert_eq!(
            steps[0].dest.as_bytes(),
            &b"state/devsvc/sdk/linux-x64/metadata"[..]
        );
        assert_eq!(
            steps[1].dest.as_bytes(),
            &b"state/devsvc/sdk/linux-x64/builder.imgref"[..]
        );
        assert!(matches!(steps[0].source, PlanSource::CopyFrom(_)));
        assert!(matches!(steps[1].source, PlanSource::ImageRef(24)));
    }

    #[test]
    fn plan_carries_checksum_flag() {
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let toolchain = slot_with_payload(
            b"linux-x64",
            "metadata@path:packages/d/sdk/linux/README.txt",
            Some(hex),
        );
        let mut steps = [PlanStep {
            dest: FixedBytes::empty(),
            source: PlanSource::CopyFrom(FixedBytes::empty()),
            verify_checksum: false,
        }; MAX_PAYLOADS];
        assert_eq!(materialize_plan(&toolchain, &mut steps), 1);
        assert!(steps[0].verify_checksum);
    }

    #[test]
    fn ref_formatting_round_trips() {
        for text in ["builder@image:24", "meta@path:packages/x/r.txt"] {
            let entry = parse_payload_entry(text).unwrap();
            let mut buffer = [0u8; MAX_PATH];
            let rendered = format_ref(&entry, &mut buffer);
            assert_eq!(str::from_utf8(rendered).unwrap(), text);
        }
    }

    #[test]
    fn join_path_rejects_overflow() {
        let mut tiny = [0u8; 16];
        assert_eq!(join_path(SDK_STATE_PREFIX, &[&[b'x'; 40]], &mut tiny), 0);
        let mut out = [0u8; 64];
        // prefix(16) + "/a"(2) + "/bc"(3)
        assert_eq!(join_path(SDK_STATE_PREFIX, &[b"a", b"bc"], &mut out), 21);
    }
}
