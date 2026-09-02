//! ChaCha20 stream cipher (RFC 8439 §2), pure `core`, no heap.
//!
//! 20-round (10 double-round) permutation on 16 u32 words with the
//! "expand 32-byte k" constants. All rotates and adds are constant-time by
//! construction; block indexing depends only on public data (counter,
//! lengths).

/// One ChaCha20 block: 64 bytes of keystream for `key`/`counter`/`nonce`
/// (RFC 8439 §2.3).
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [
        0x6170_7865,
        0x3320_646e,
        0x7962_2d32,
        0x6b20_6574,
        u32::from_le_bytes(key[0..4].try_into().unwrap()),
        u32::from_le_bytes(key[4..8].try_into().unwrap()),
        u32::from_le_bytes(key[8..12].try_into().unwrap()),
        u32::from_le_bytes(key[12..16].try_into().unwrap()),
        u32::from_le_bytes(key[16..20].try_into().unwrap()),
        u32::from_le_bytes(key[20..24].try_into().unwrap()),
        u32::from_le_bytes(key[24..28].try_into().unwrap()),
        u32::from_le_bytes(key[28..32].try_into().unwrap()),
        counter,
        u32::from_le_bytes(nonce[0..4].try_into().unwrap()),
        u32::from_le_bytes(nonce[4..8].try_into().unwrap()),
        u32::from_le_bytes(nonce[8..12].try_into().unwrap()),
    ];
    let working = state;
    for _ in 0..10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..(i + 1) * 4].copy_from_slice(&state[i].wrapping_add(working[i]).to_le_bytes());
    }
    out
}

/// The ChaCha20 quarter round (RFC 8439 §2.1.1) on state indices
/// `a, b, c, d`.
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// XOR `data` with the ChaCha20 keystream starting at block `counter`,
/// writing into `out` (which must be at least as long as `data`). Encrypt
/// and decrypt are the same operation.
pub fn xor(key: &[u8; 32], counter: u32, nonce: &[u8; 12], data: &[u8], out: &mut [u8]) {
    debug_assert!(out.len() >= data.len());
    for (chunk_idx, chunk) in data.chunks(64).enumerate() {
        let ks = block(key, counter.wrapping_add(chunk_idx as u32), nonce);
        for (i, byte) in chunk.iter().enumerate() {
            out[chunk_idx * 64 + i] = byte ^ ks[i];
        }
    }
}

#[cfg(test)]
mod tests_chacha20 {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut o = [0u8; N];
        for i in 0..N {
            o[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        o
    }

    /// RFC 8439 §2.4.2: test vector for the ChaCha20 block function
    /// (key 0x00..0x1f, counter 1, nonce 0x00..00 09 00..4a 00..00).
    #[test]
    fn rfc8439_2_4_2_block_function() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let nonce = unhex::<12>("000000090000004a00000000");
        let ks = block(&key, 1, &nonce);
        assert_eq!(
            ks,
            unhex::<64>(
                "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
                 d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
            )
        );
    }

    /// RFC 8439 §2.4.2 (and Appendix A.1): full "sunscreen" encryption with
    /// counter 1.
    #[test]
    fn rfc8439_2_4_2_sunscreen_encryption() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let nonce = unhex::<12>("000000000000004a00000000");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you \
only one tip for the future, sunscreen would be it.";
        let mut ct = [0u8; 114];
        xor(&key, 1, &nonce, plaintext, &mut ct);
        assert_eq!(
            ct,
            unhex::<114>(
                "6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0\
                 bf91b65c5524733ab8f593dabcd62b3571639d624e65152ab8f530c359f0861d\
                 807ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab779373\
                 65af90bbf74a35be6b40b8eedf2785e42874d"
            )
        );
        // Decrypting the ciphertext reproduces the plaintext.
        let mut back = [0u8; 114];
        xor(&key, 1, &nonce, &ct, &mut back);
        assert_eq!(back, *plaintext);
    }
}
