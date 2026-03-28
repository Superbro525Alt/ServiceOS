use serviceos_kernel_core::bootstrap::BootInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootSupportStatus {
    pub boot_info_ready: bool,
}

pub fn boot_support_status() -> BootSupportStatus {
    BootSupportStatus {
        boot_info_ready: false,
    }
}

pub fn capture_boot_info() -> Option<BootInfo<'static>> {
    None
}
