#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferStatus {
    pub implemented: bool,
}

pub const fn status() -> FramebufferStatus {
    FramebufferStatus { implemented: false }
}
