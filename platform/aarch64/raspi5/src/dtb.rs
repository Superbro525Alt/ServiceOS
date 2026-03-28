#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceTreeStatus {
    pub parser_ready: bool,
}

pub const fn status() -> DeviceTreeStatus {
    DeviceTreeStatus {
        parser_ready: false,
    }
}
