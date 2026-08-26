use core::fmt::Write;

use serviceos_userspace_runtime::FixedLogBuffer;

use crate::MAX_NOTIFICATION_BYTES;

/// Packed user-fault exit-word tag. Mirrors the additive kernel packing in
/// kernel/core/src/fault.rs (tag bits 63..48, low-32 address bits 47..16,
/// class nibble 15..12, legacy detail 11..0) so the desktop can decode the
/// same words root-manager and the supervisor see.
pub(crate) const USER_FAULT_EXIT_TAG: u64 = 0xf100_0000_0000_0000;

/// Why a managed app crashed, decoded from the exit-word class nibble.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CrashClass {
    Unknown,
    NullDeref,
    WildAddress,
    ExecNonExec,
    Permission,
}

impl CrashClass {
    pub(crate) const fn from_code(code: u64) -> Self {
        match code & 0xf {
            1 => Self::NullDeref,
            2 => Self::WildAddress,
            3 => Self::ExecNonExec,
            4 => Self::Permission,
            _ => Self::Unknown,
        }
    }

    #[cfg(test)]
    pub(crate) const fn code(self) -> u64 {
        match self {
            Self::Unknown => 0,
            Self::NullDeref => 1,
            Self::WildAddress => 2,
            Self::ExecNonExec => 3,
            Self::Permission => 4,
        }
    }

    /// Plain-language line shown to the user in the crash notification.
    pub(crate) const fn explanation(self) -> &'static str {
        match self {
            Self::Unknown => "hit an unrecoverable fault",
            Self::NullDeref => "used a null or near-null pointer",
            Self::WildAddress => "followed a wild pointer to unmapped memory",
            Self::ExecNonExec => "jumped to non-executable memory",
            Self::Permission => "touched memory without permission",
        }
    }
}

/// Decoded user-fault details from a packed exit word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CrashFault {
    pub(crate) class: CrashClass,
    /// Low 32 bits of the faulting address (page faults) or instruction
    /// pointer (other exceptions), exactly as packed by the kernel.
    pub(crate) address: u64,
}

pub(crate) fn decode_user_fault_exit(exit_code: u64) -> Option<CrashFault> {
    if exit_code & 0xffff_0000_0000_0000 != USER_FAULT_EXIT_TAG {
        return None;
    }
    Some(CrashFault {
        class: CrashClass::from_code((exit_code >> 12) & 0xf),
        address: (exit_code >> 16) & 0xffff_ffff,
    })
}

/// Assembles the crash-notification payload: "App crashed — <title>:
/// <class explanation>, address 0x…". Truncates like every other fixed
/// notification buffer rather than panicking.
pub(crate) fn crash_notification_text(
    app_title: &str,
    fault: &CrashFault,
) -> FixedLogBuffer<MAX_NOTIFICATION_BYTES> {
    let mut text = FixedLogBuffer::<MAX_NOTIFICATION_BYTES>::new();
    let _ = write!(
        &mut text,
        "App crashed \u{2014} {}: {}, address {:#x}",
        app_title, fault.class.explanation(), fault.address
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAG: u64 = USER_FAULT_EXIT_TAG;

    fn pack(detail: u64, class: CrashClass, address_or_ip: u64) -> u64 {
        TAG | ((address_or_ip & 0xffff_ffff) << 16) | (class.code() << 12) | (detail & 0xfff)
    }

    #[test]
    fn class_codes_roundtrip_and_match_kernel_names() {
        for (code, expected) in [
            (0u64, CrashClass::Unknown),
            (1, CrashClass::NullDeref),
            (2, CrashClass::WildAddress),
            (3, CrashClass::ExecNonExec),
            (4, CrashClass::Permission),
            (5, CrashClass::Unknown),
            (0xf, CrashClass::Unknown),
        ] {
            assert_eq!(CrashClass::from_code(code), expected);
            if expected != CrashClass::Unknown || code == 0 {
                assert_eq!(expected.code() & 0xf, code.min(4));
            }
        }
        // Nibble masking mirrors FaultClass::from_code(& 0xf).
        assert_eq!(CrashClass::from_code(0x13), CrashClass::ExecNonExec);
        assert_eq!(CrashClass::from_code(0x22), CrashClass::WildAddress);
    }

    #[test]
    fn explanations_are_plain_language_per_class() {
        assert_eq!(
            CrashClass::NullDeref.explanation(),
            "used a null or near-null pointer"
        );
        assert_eq!(
            CrashClass::WildAddress.explanation(),
            "followed a wild pointer to unmapped memory"
        );
        assert_eq!(
            CrashClass::ExecNonExec.explanation(),
            "jumped to non-executable memory"
        );
        assert_eq!(
            CrashClass::Permission.explanation(),
            "touched memory without permission"
        );
        assert_eq!(
            CrashClass::Unknown.explanation(),
            "hit an unrecoverable fault"
        );
    }

    #[test]
    fn decode_matches_kernel_exit_word_packing() {
        let fault = decode_user_fault_exit(pack(0x102, CrashClass::NullDeref, 0x8))
            .expect("packed user-fault word");
        assert_eq!(fault.class, CrashClass::NullDeref);
        assert_eq!(fault.address, 0x8);

        let fault = decode_user_fault_exit(pack(0x300 | 14, CrashClass::ExecNonExec, 0x0040_1000))
            .expect("exec-nonexec word");
        assert_eq!(fault.class, CrashClass::ExecNonExec);
        assert_eq!(fault.address, 0x40_1000);

        // High address bits are dropped exactly as the kernel packs them.
        let fault = decode_user_fault_exit(pack(2, CrashClass::WildAddress, 0xdead_beef_0007_fffc))
            .expect("wild-address word");
        assert_eq!(fault.address, 0x0007_fffc);

        // Legacy-compatible words (tag set, zero class) decode as unknown
        // with whatever address bits the old encoding happened to carry.
        let fault = decode_user_fault_exit(TAG | 0x30e).expect("legacy-shaped word");
        assert_eq!(fault.class, CrashClass::Unknown);
        assert_eq!(fault.address, 0);

        // Non-fault exits never decode.
        assert!(decode_user_fault_exit(0).is_none());
        assert!(decode_user_fault_exit(0xf670).is_none());
        assert!(decode_user_fault_exit(3).is_none());
    }

    #[test]
    fn notification_payload_assembles_from_packed_word_decode() {
        let exit_word = pack(0x104, CrashClass::Permission, 0x2000);
        let fault = decode_user_fault_exit(exit_word).expect("fault");
        let text = crash_notification_text("Monitor", &fault);
        let body = core::str::from_utf8(text.as_bytes()).expect("utf-8 payload");
        assert!(body.starts_with("App crashed \u{2014} Monitor: "));
        assert!(body.contains("touched memory without permission"));
        assert!(body.ends_with(", address 0x2000"));

        let exec = crash_notification_text(
            "Terminal",
            &decode_user_fault_exit(pack(6, CrashClass::ExecNonExec, 0x401000)).expect("fault"),
        );
        assert!(core::str::from_utf8(exec.as_bytes())
            .expect("utf-8")
            .contains("Terminal: jumped to non-executable memory, address 0x401000"));
    }

    #[test]
    fn payload_truncates_inside_the_fixed_buffer() {
        let fault = CrashFault {
            class: CrashClass::WildAddress,
            address: 0xffff_fffc,
        };
        let text = crash_notification_text("Software Center", &fault);
        assert!(text.as_bytes().len() <= MAX_NOTIFICATION_BYTES);
        assert!(core::str::from_utf8(text.as_bytes()).is_ok());
    }
}
