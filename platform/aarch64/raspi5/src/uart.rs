#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UartStatus {
    pub implemented: bool,
}

pub const fn status() -> UartStatus {
    UartStatus { implemented: false }
}
