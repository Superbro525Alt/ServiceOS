use x86_64::registers::control::{Cr0, Cr0Flags, Cr2};

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
