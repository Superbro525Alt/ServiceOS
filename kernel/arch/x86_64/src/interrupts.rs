/// Phase 0 placeholder for x86_64 descriptor-table state.
///
/// This remains intentionally small until the exception path and timer source
/// are implemented for real.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorState {
    pub idt_loaded: bool,
    pub tss_loaded: bool,
}

impl DescriptorState {
    pub const fn uninitialized() -> Self {
        Self {
            idt_loaded: false,
            tss_loaded: false,
        }
    }
}
