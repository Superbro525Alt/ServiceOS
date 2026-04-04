#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleTag {
    WriteRecord = 0x200,
    SessionOpenRequest = 0x201,
    SessionOpenReply = 0x202,
    SessionWriteText = 0x203,
    SessionReadLineRequest = 0x204,
    SessionReadLineReply = 0x205,
}
