use serviceos_kernel_core::memory::PhysicalAddress;
use x86_64::{
    PhysAddr,
    registers::control::{Cr0, Cr0Flags, Cr2, Cr3, Cr3Flags},
    structures::paging::PhysFrame,
};

pub fn disable_interrupts() {
    x86_64::instructions::interrupts::disable();
}

pub fn enable_interrupts() {
    x86_64::instructions::interrupts::enable();
}

pub fn interrupts_enabled() -> bool {
    x86_64::instructions::interrupts::are_enabled()
}

pub fn halt() {
    x86_64::instructions::hlt();
}

pub fn halt_loop() -> ! {
    loop {
        halt();
    }
}

pub fn with_interrupts_disabled<R>(f: impl FnOnce() -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(f)
}

pub fn with_write_protect_disabled<R>(f: impl FnOnce() -> R) -> R {
    let original = Cr0::read();

    unsafe {
        Cr0::update(|flags| flags.remove(Cr0Flags::WRITE_PROTECT));
    }

    let result = f();

    unsafe {
        Cr0::write(original);
    }

    result
}

pub fn read_page_fault_address() -> u64 {
    Cr2::read_raw()
}

pub fn current_page_table_root() -> PhysicalAddress {
    let (root, _) = Cr3::read();
    PhysicalAddress::new(root.start_address().as_u64())
}

pub unsafe fn load_page_table_root(root: PhysicalAddress) {
    let frame = PhysFrame::from_start_address(PhysAddr::new(root.as_u64()))
        .expect("page table roots are always page aligned");
    unsafe {
        Cr3::write(frame, Cr3Flags::empty());
    }
}
