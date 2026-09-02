//! Error and disconnect vocabulary (RFC 4253 §11.1).

/// SSH disconnect reason codes this transport produces or consumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum DisconnectReason {
    HostNotPermitted = 1,
    ProtocolError = 2,
    KeyExchangeFailed = 3,
    MacError = 5,
    CompressionError = 6,
    ServiceNotAvailable = 7,
    ProtocolVersionNotSupported = 8,
    HostKeyNotVerifiable = 9,
    ByApplication = 11,
}

impl DisconnectReason {
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Map a peer-sent reason code; unknown codes are preserved by the
    /// caller as raw u32 (they are logged, not interpreted).
    pub fn from_code(code: u32) -> Option<DisconnectReason> {
        Some(match code {
            1 => DisconnectReason::HostNotPermitted,
            2 => DisconnectReason::ProtocolError,
            3 => DisconnectReason::KeyExchangeFailed,
            5 => DisconnectReason::MacError,
            6 => DisconnectReason::CompressionError,
            7 => DisconnectReason::ServiceNotAvailable,
            8 => DisconnectReason::ProtocolVersionNotSupported,
            9 => DisconnectReason::HostKeyNotVerifiable,
            11 => DisconnectReason::ByApplication,
            _ => return None,
        })
    }
}

/// Terminal failure of a transport. Every variant ends the connection; the
/// transport state moves to [`crate::transport::State::Closed`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fail {
    /// This side detected a violation: a DISCONNECT packet with the given
    /// reason has been queued to the output buffer before the error returns.
    LocalDisconnect {
        reason: DisconnectReason,
        description: &'static str,
    },
    /// The peer sent SSH_MSG_DISCONNECT (reason code is passed through
    /// verbatim; the description is retrievable from the transport).
    PeerDisconnect { reason_code: u32 },
    /// The output buffer would overflow; drain `pending_output()` and retry
    /// the operation. The connection is NOT closed by this error.
    OutOfCapacity,
    /// Operation invalid in the current state (e.g. `send_payload` before
    /// the transport is established). The connection is NOT closed.
    NotReady,
    /// feed/send called after the transport closed.
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_roundtrip() {
        for code in [1u32, 2, 3, 5, 6, 7, 8, 9, 11] {
            let r = DisconnectReason::from_code(code).expect("known code");
            assert_eq!(r.code(), code);
        }
        assert_eq!(DisconnectReason::from_code(4), None);
        assert_eq!(DisconnectReason::from_code(0), None);
    }

    #[test]
    fn fail_is_copy() {
        let f = Fail::Closed;
        let g = f;
        assert_eq!(f, g);
    }
}
