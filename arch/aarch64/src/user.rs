#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserBringupStatus {
    pub transition_path: bool,
}

pub const fn bringup_status() -> UserBringupStatus {
    UserBringupStatus {
        transition_path: false,
    }
}
