#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserBringupStatus {
    pub implemented: bool,
}

pub const fn bringup_status() -> UserBringupStatus {
    UserBringupStatus { implemented: false }
}
