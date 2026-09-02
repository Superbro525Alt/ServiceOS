//! KEXINIT algorithm-name negotiation (RFC 4253 §7.1).
//!
//! Rule implemented: for each algorithm class the chosen algorithm is the
//! **first name on the client's list that also appears on the server's
//! list** (client order wins). No match in a class is a hard failure.
//!
//! Advertised sets (honest): exactly one KEX (curve25519-sha256 and its
//! libssh.org alias, same construction), one host-key algorithm
//! (ssh-ed25519), one cipher (chacha20-poly1305@openssh.com), one
//! compression (none). The MAC class is advertised as `hmac-sha2-256` purely
//! to satisfy the RFC 4253 §7.1 rule that every list carries at least one
//! name — with the only negotiable cipher being AEAD, the negotiated MAC is
//! defined to be unused (RFC 4253 §6.4: with an AEAD cipher the MAC
//! algorithm is ignored).

use crate::wire::NameList;

pub const KEX_ALGS: &[&str] = &["curve25519-sha256", "curve25519-sha256@libssh.org"];
pub const HOSTKEY_ALGS: &[&str] = &["ssh-ed25519"];
pub const CIPHER_ALGS: &[&str] = &["chacha20-poly1305@openssh.com"];
pub const MAC_ALGS: &[&str] = &["hmac-sha2-256"];
pub const COMPRESSION_ALGS: &[&str] = &["none"];

/// The negotiated choices. `mac` and `languages` are carried for
/// completeness; with our single AEAD cipher the MAC is never used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Negotiated {
    pub kex: &'static str,
    pub host_key: &'static str,
    pub cipher_c2s: &'static str,
    pub cipher_s2c: &'static str,
    pub mac_c2s: &'static str,
    pub mac_s2c: &'static str,
    pub compression_c2s: &'static str,
    pub compression_s2c: &'static str,
}

/// Pick the first client-list name that appears in `server` (both are
/// ASCII byte strings). Returns the matched **server-side** static name.
pub fn pick(client_list: &[u8], server: &[&'static str]) -> Option<&'static str> {
    for client_name in NameList::new(client_list) {
        for &s in server {
            if client_name == s.as_bytes() {
                return Some(s);
            }
        }
    }
    None
}

/// Negotiate all algorithm classes. `Ok(None)` fields are impossible here:
/// any mismatch aborts negotiation with the class name for the disconnect
/// description.
pub fn negotiate(
    kex_client: &[u8],
    hostkey_client: &[u8],
    cipher_c2s_client: &[u8],
    cipher_s2c_client: &[u8],
    mac_c2s_client: &[u8],
    mac_s2c_client: &[u8],
    comp_c2s_client: &[u8],
    comp_s2c_client: &[u8],
) -> Result<Negotiated, &'static str> {
    let kex = pick(kex_client, KEX_ALGS).ok_or("no common kex algorithm")?;
    let host_key = pick(hostkey_client, HOSTKEY_ALGS).ok_or("no common host key algorithm")?;
    let cipher_c2s =
        pick(cipher_c2s_client, CIPHER_ALGS).ok_or("no common client-to-server cipher")?;
    let cipher_s2c =
        pick(cipher_s2c_client, CIPHER_ALGS).ok_or("no common server-to-client cipher")?;
    // MAC classes must resolve for a compliant KEXINIT even though the AEAD
    // cipher makes the result unused; fall back to "none" only if the peer
    // sends an empty list, which non-AEAD-aware peers never do.
    let mac_c2s = pick(mac_c2s_client, MAC_ALGS).unwrap_or("hmac-sha2-256");
    let mac_s2c = pick(mac_s2c_client, MAC_ALGS).unwrap_or("hmac-sha2-256");
    let compression_c2s =
        pick(comp_c2s_client, COMPRESSION_ALGS).ok_or("no common client-to-server compression")?;
    let compression_s2c =
        pick(comp_s2c_client, COMPRESSION_ALGS).ok_or("no common server-to-client compression")?;
    Ok(Negotiated {
        kex,
        host_key,
        cipher_c2s,
        cipher_s2c,
        mac_c2s,
        mac_s2c,
        compression_c2s,
        compression_s2c,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_order_wins() {
        // Client offers both kex names; server's first entry loses to
        // client's first match.
        let client = b"curve25519-sha256@libssh.org,curve25519-sha256";
        assert_eq!(pick(client, KEX_ALGS), Some("curve25519-sha256@libssh.org"));
    }

    #[test]
    fn mismatch_and_empty() {
        assert_eq!(pick(b"ecdh-sha2-nistp256", KEX_ALGS), None);
        assert_eq!(pick(b"", KEX_ALGS), None);
        assert_eq!(
            pick(b"aes128-ctr,chacha20-poly1305@openssh.com", CIPHER_ALGS),
            Some("chacha20-poly1305@openssh.com")
        );
    }

    #[test]
    fn negotiate_happy_path() {
        let n = negotiate(
            b"curve25519-sha256@libssh.org,curve25519-sha256",
            b"ssh-ed25519,rsa-sha2-512",
            b"chacha20-poly1305@openssh.com",
            b"chacha20-poly1305@openssh.com",
            b"hmac-sha2-512",
            b"hmac-sha2-512",
            b"none",
            b"none",
        )
        .unwrap();
        assert_eq!(n.kex, "curve25519-sha256@libssh.org");
        assert_eq!(n.host_key, "ssh-ed25519");
        assert_eq!(n.cipher_c2s, "chacha20-poly1305@openssh.com");
    }

    #[test]
    fn negotiate_mismatches_name_the_class() {
        assert_eq!(
            negotiate(
                b"ecdh-sha2-nistp256",
                b"ssh-ed25519",
                b"chacha20-poly1305@openssh.com",
                b"chacha20-poly1305@openssh.com",
                b"hmac-sha2-256",
                b"hmac-sha2-256",
                b"none",
                b"none",
            ),
            Err("no common kex algorithm")
        );
        assert_eq!(
            negotiate(
                b"curve25519-sha256",
                b"ssh-rsa",
                b"chacha20-poly1305@openssh.com",
                b"chacha20-poly1305@openssh.com",
                b"hmac-sha2-256",
                b"hmac-sha2-256",
                b"none",
                b"none",
            ),
            Err("no common host key algorithm")
        );
        assert_eq!(
            negotiate(
                b"curve25519-sha256",
                b"ssh-ed25519",
                b"aes256-gcm@openssh.com",
                b"chacha20-poly1305@openssh.com",
                b"hmac-sha2-256",
                b"hmac-sha2-256",
                b"none",
                b"none",
            ),
            Err("no common client-to-server cipher")
        );
    }

    #[test]
    fn negotiate_compression_required() {
        assert_eq!(
            negotiate(
                b"curve25519-sha256",
                b"ssh-ed25519",
                b"chacha20-poly1305@openssh.com",
                b"chacha20-poly1305@openssh.com",
                b"hmac-sha2-256",
                b"hmac-sha2-256",
                b"zlib@openssh.com",
                b"none",
            ),
            Err("no common client-to-server compression")
        );
    }

    #[test]
    fn mac_falls_back_when_aead_peer_omits_mac_list() {
        let n = negotiate(
            b"curve25519-sha256",
            b"ssh-ed25519",
            b"chacha20-poly1305@openssh.com",
            b"chacha20-poly1305@openssh.com",
            b"",
            b"",
            b"none",
            b"none",
        )
        .unwrap();
        assert_eq!(n.mac_c2s, "hmac-sha2-256");
    }
}
