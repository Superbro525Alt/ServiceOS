#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirmwareStatus {
    pub config_template_ready: bool,
}

pub const fn status() -> FirmwareStatus {
    FirmwareStatus {
        config_template_ready: true,
    }
}
