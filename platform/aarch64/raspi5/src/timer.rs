#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerStatus {
    pub implemented: bool,
}

pub const fn status() -> TimerStatus {
    TimerStatus { implemented: false }
}
