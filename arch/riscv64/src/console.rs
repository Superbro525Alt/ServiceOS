//! Serial console output through the SBI legacy console.

use core::fmt;

use crate::sbi;

pub struct SbiConsole;

impl SbiConsole {
    pub const fn new() -> Self {
        Self
    }

    pub fn write_str(&mut self, text: &str) {
        for byte in text.bytes() {
            if byte == b'\n' {
                sbi::console_putchar(b'\r');
            }
            sbi::console_putchar(byte);
        }
    }
}

impl fmt::Write for SbiConsole {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        SbiConsole::write_str(self, text);
        Ok(())
    }
}

#[macro_export]
macro_rules! sbi_println {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let mut console = $crate::console::SbiConsole::new();
        let _ = writeln!(console, $($arg)*);
    }};
}
