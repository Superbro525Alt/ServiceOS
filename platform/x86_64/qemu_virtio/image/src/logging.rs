use core::{fmt, str};

use serviceos_platform_qemu_virtio::serial;

pub(crate) fn debug_log_writer(bytes: &[u8]) {
    if let Ok(text) = str::from_utf8(bytes) {
        log("service", format_args!("{text}"));
    } else {
        log("service", format_args!("<non-utf8 {} bytes>", bytes.len()));
    }
}

pub(crate) fn log_line(domain: &str, message: &str) {
    serial::write_args(format_args!("serviceos: {domain}: {message}\n"));
}

pub(crate) fn log(domain: &str, args: fmt::Arguments<'_>) {
    serial::write_args(format_args!("serviceos: {domain}: {args}\n"));
}
