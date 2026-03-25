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
