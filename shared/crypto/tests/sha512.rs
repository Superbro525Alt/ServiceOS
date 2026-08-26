//! FIPS 180-4 / known-answer vectors for SHA-512.

use serviceos_crypto::sha512::digest;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[test]
fn sha512_empty() {
    assert_eq!(
        hex(&digest(&[b""])),
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
}

#[test]
fn sha512_abc() {
    assert_eq!(
        hex(&digest(&[b"abc"])),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
}

#[test]
fn sha512_two_block_message() {
    // FIPS 180-4 example: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
    assert_eq!(
        hex(&digest(&[b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"])),
        "204a8fc6dda82f0a0ced7beb8e08a41657c16ef468b228a8279be331a703c33596fd15c13b1b07f9aa1d3bea57789ca031ad85c7a71dd70354ec631238ca3445"
    );
}

#[test]
fn sha512_streaming_matches_one_shot() {
    let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let one_shot = digest(&[&data]);
    let mut h = serviceos_crypto::sha512::Sha512::new();
    // Odd chunk sizes exercise buffering across the 128-byte block edge.
    for chunk in data.chunks(37) {
        h.update(chunk);
    }
    assert_eq!(h.finalize(), one_shot);
}
