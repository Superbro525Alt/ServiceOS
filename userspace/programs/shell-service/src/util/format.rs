use core::fmt::Write;

use rt::ConfigKey;
use serviceos_userspace_runtime as rt;

pub(crate) fn config_value_text(key: ConfigKey, value: u64) -> FixedValueText {
    match key {
        ConfigKey::NetworkIpv4Address
        | ConfigKey::NetworkIpv4Gateway
        | ConfigKey::NetworkDnsServer => FixedValueText::ipv4(value as u32),
        _ => FixedValueText::unsigned(value),
    }
}

pub(crate) fn format_ipv4(value: u32) -> FixedValueText {
    FixedValueText::ipv4(value)
}

pub(crate) fn format_mac(value: [u8; 6]) -> FixedValueText {
    FixedValueText::mac(value)
}

pub(crate) fn unpack_mac(value: u64) -> [u8; 6] {
    [
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 24) & 0xff) as u8,
        ((value >> 32) & 0xff) as u8,
        ((value >> 40) & 0xff) as u8,
    ]
}

pub(crate) struct FixedValueText {
    bytes: [u8; 32],
    len: usize,
}

impl FixedValueText {
    pub(crate) const fn empty() -> Self {
        Self {
            bytes: [0; 32],
            len: 0,
        }
    }

    pub(crate) fn unsigned(value: u64) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(&mut text, "{value}");
        text
    }

    pub(crate) fn ipv4(value: u32) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{}.{}.{}.{}",
            (value >> 24) & 0xff,
            (value >> 16) & 0xff,
            (value >> 8) & 0xff,
            value & 0xff,
        );
        text
    }

    pub(crate) fn mac(value: [u8; 6]) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            value[0], value[1], value[2], value[3], value[4], value[5],
        );
        text
    }
}

impl core::fmt::Display for FixedValueText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = core::str::from_utf8(&self.bytes[..self.len]).map_err(|_| core::fmt::Error)?;
        f.write_str(text)
    }
}

impl Write for FixedValueText {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let bytes = value.as_bytes();
        let remaining = self.bytes.len().saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

pub(crate) fn printable_version(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}
