#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkStatus {
    pub implemented: bool,
}

pub const fn status() -> NetworkStatus {
    NetworkStatus { implemented: false }
}
