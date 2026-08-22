use serviceos_kernel_arch_aarch64::gic;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicStatus {
    pub implemented: bool,
}

pub fn status() -> GicStatus {
    GicStatus {
        implemented: gic::is_active(),
    }
}
