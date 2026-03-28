#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockStatus {
    pub implemented: bool,
}

pub const fn status() -> BlockStatus {
    BlockStatus { implemented: false }
}
