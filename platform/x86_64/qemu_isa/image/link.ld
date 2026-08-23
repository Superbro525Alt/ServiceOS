ENTRY(_start)

SECTIONS
{
    . = 0x00000000007C00;

    .stage1 : ALIGN(1) {
        KEEP(*(.text.stage1))
    }
    ASSERT(SIZEOF(.stage1) <= 510, "stage1 exceeds 510 bytes")

    . = 0x00000000007DFE;
    .stage1_sig : {
        SHORT(0xAA55)
    }

    . = 0x0000000000100000;
    __payload_start = .;

    .text : ALIGN(16) {
        *(.text .text.*)
    }

    /* PVH note must survive --gc-sections AND the /DISCARD rule below:
       QEMU's -kernel loader keys off this SHT_NOTE for 64-bit images.
       Placed AFTER .text so it cannot overlap the entry code. */
    .pvh.note : { KEEP(*(.pvh.note)) }

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

    __payload_end = .;
    __payload_sectors = (__payload_end - __payload_start + 511) / 512;

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
        *(.note .note.*)
        *(.comment)
    }
}
