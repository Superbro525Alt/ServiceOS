ENTRY(_start)

SECTIONS
{
    /* QEMU virt DRAM base is 0x80000000; OpenSBI (-bios default) occupies
       the first 2 MiB and jumps to 0x80200000, so the payload links there. */
    . = 0x0000000080200000;
    __image_start = .;

    .text : ALIGN(16) {
        KEEP(*(.text._start))
        *(.text .text.*)
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(16) {
        *(.data .data.*)
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        __bss_end = .;
    }

    .boot_stack (NOLOAD) : ALIGN(16) {
        __boot_stack_bottom = .;
        KEEP(*(.boot_stack))
        __boot_stack_top = .;
    }

    __image_end = .;
    __kernel_image_size = __image_end - __image_start;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
