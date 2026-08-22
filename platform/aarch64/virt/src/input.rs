#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputStatus {
    pub implemented: bool,
}

pub const fn status() -> InputStatus {
    InputStatus { implemented: false }
}
