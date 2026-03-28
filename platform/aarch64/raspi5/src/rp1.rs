#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rp1Status {
    pub implemented: bool,
}

pub const fn status() -> Rp1Status {
    Rp1Status { implemented: false }
}
