use x86_64::registers::control::{Cr0, Cr0Flags};

pub fn disable_interrupts() {
    x86_64::instructions::interrupts::disable();
}

pub fn halt() {
    x86_64::instructions::hlt();
}

pub fn halt_loop() -> ! {
    loop {
        halt();
    }
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
