ENTRY(_start)

SECTIONS
{
    . = 0x0000000040080000;
    __image_start = .;

    .image_header : ALIGN(16) {
        KEEP(*(.image.header))
    }

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
        *(.boot_stack)
    }

    __image_end = .;
    __kernel_image_size = __image_end - __image_start;
}
