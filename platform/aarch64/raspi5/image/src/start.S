.section .text._start, "ax"
.global _start

_start:
    mrs x1, mpidr_el1
    and x1, x1, #0xff
    cbz x1, 1f
0:
    wfe
    b 0b

1:
    adrp x1, BOOT_STACK
    add x1, x1, :lo12:BOOT_STACK
    add x1, x1, #65536
    mov sp, x1

    adrp x2, __bss_start
    add x2, x2, :lo12:__bss_start
    adrp x3, __bss_end
    add x3, x3, :lo12:__bss_end

2:
    cmp x2, x3
    b.hs 3f
    str xzr, [x2], #8
    b 2b

3:
    bl serviceos_raspi5_entry

4:
    wfe
    b 4b
