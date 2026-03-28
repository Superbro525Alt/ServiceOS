.section .text._start, "ax"
.global _start

_start:
    mov x20, x0
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
    mrs x1, CurrentEL
    cmp x1, #(2 << 2)
    b.ne 4f
    mov x1, #3
    msr cnthctl_el2, x1
    msr cntvoff_el2, xzr
    mov x1, #(1 << 31)
    msr hcr_el2, x1
    mov x1, sp
    msr sp_el1, x1
    adrp x1, 4f
    add x1, x1, :lo12:4f
    msr elr_el2, x1
    mov x1, #0x3c5
    msr spsr_el2, x1
    eret

4:
    mov x0, x20
    bl serviceos_raspi5_entry

5:
    wfe
    b 5b
