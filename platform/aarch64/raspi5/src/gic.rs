#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicStatus {
    pub implemented: bool,
}

pub const fn status() -> GicStatus {
    GicStatus { implemented: false }
}
