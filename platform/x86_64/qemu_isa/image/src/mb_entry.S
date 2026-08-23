# ServiceOS qemu-isa entry: PVH ELF note + long-mode trampoline.
# QEMU -kernel recognizes a 64-bit ELF carrying the Xen PVH PHYS32_ENTRY
# note, loads every segment, enters this stub in 32-bit protected mode with
# EBX = &hvm_start_info. Everything else is parsed in Rust (see isa_entry).

# Fixed low-memory layout. NOTE: QEMU's -kernel ELF loader places the
# image's own segments in low RAM (this image's headers segment lands at
# PA 0x7000) and re-restores them on every reset -- so page tables MUST
# stay clear of 0x7000..0x9000 and of the payload. 0x30000.. is unused
# conventional RAM on the pc machine.
.set PML4_ADDR, 0x30000
.set PDPT_ADDR, 0x31000
.set PD_BASE,   0x32000    # four consecutive PDs cover 0..4GiB
.set SEL_CS32,  0x18

.section .pvh.note, "a", @note
.align 4
.long 4                       # namesz("Xen" + NUL)
.long 4                       # descsz(u32 phys32 entry)
.long 18                      # type = XEN_ELFNOTE_PHYS32_ENTRY
.asciz "Xen"
.long pvh_phys32_entry
.align 4

.section .text._start, "ax"
.code32
.globl _start
_start:
pvh_phys32_entry:
    cli
    cld
    # EBX holds &hvm_start_info for the whole stub; never clobber it here.
    movl $boot_stack_top, %esp   # our own stack, identity-mapped once PG is on
    movb $'A', %al
    movw $0x3F8, %dx
    outb %al, %dx

    # Identity map the first 4GiB with 2MiB pages. The kernel's interrupt
    # path programs the local APIC at 0xFEE00000 and the PIC path expects
    # firmware-style full low-4G coverage (OVMF provides the same on the
    # UEFI platform), so one GiB is not enough.
    # NOTE: EBX carries &hvm_start_info into long mode -- never touch it.
    movl $(PDPT_ADDR + 3), PML4_ADDR   # PML4[0] -> PDPT
    movb $'F', %al
    movw $0x3F8, %dx
    outb %al, %dx
    # PDPT[0..3] -> four page directories.
    movl $PDPT_ADDR, %edi
    movl $PD_BASE, %esi
    addl $3, %esi               # present | writable
    movl $4, %ecx
.fill_pdpt_loop:
    movl %esi, (%edi)
    addl $0x1000, %esi          # next PD frame
    addl $8, %edi
    loop .fill_pdpt_loop
    # Each PD: 512 x 2MiB pages covering its 1GiB slice.
    movl $PD_BASE, %edx         # current PD physical address
    xorl %edi, %edi             # current 1GiB slice frame base
    movl $4, %ecx               # four PDs
.fill_pd_outer:
    movl %edx, %esi
    movl $512, %ebp
.fill_pd_inner:
    movl %edi, %eax
    orl $0x83, %eax             # present | writable | 2MiB page
    movl %eax, (%esi)
    addl $0x200000, %edi
    addl $8, %esi
    decl %ebp
    jnz .fill_pd_inner
    addl $0x1000, %edx          # next PD table
    decl %ecx
    jnz .fill_pd_outer
    movb $'G', %al
    movw $0x3F8, %dx
    outb %al, %dx

    movb $'H', %al
    movw $0x3F8, %dx
    outb %al, %dx
    lgdt gdt_desc             # load long-mode GDT BEFORE enabling paging
    ljmp $SEL_CS32, $cs_reloaded   # reload CS from OUR GDT while still flat 32-bit
cs_reloaded:

    movb $'I', %al
    movw $0x3F8, %dx
    outb %al, %dx
    mov %cr4, %eax
    or $0x20, %eax            # PAE
    mov %eax, %cr4
    movl $PML4_ADDR, %eax
    mov %eax, %cr3
    movb $'J', %al
    movw $0x3F8, %dx
    outb %al, %dx
    mov $0xC0000080, %ecx     # EFER
    rdmsr
    or $0x900, %eax           # LME | NXE -- kernel PTEs carry the NX bit
                              # (bit 63); with NXE clear that is a reserved-
                              # bit page fault on first use of any such PTE.
    wrmsr
    movb $'K', %al
    movw $0x3F8, %dx
    outb %al, %dx
    mov %cr0, %eax
    or $0x80000001, %eax      # PG | PE  (LME set: this enters long mode)
    mov %eax, %cr0
    ljmp $0x08, $long_mode_entry   # reload CS from the L=1 GDT entry

halt32:
    cli
    hlt
    jmp halt32

.code64
long_mode_entry:
    movb $'L', %al
    movw $0x3F8, %dx
    outb %al, %dx
    movl $0x10, %eax          # data selector -- never inherit stale register bits
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %fs
    mov %ax, %gs
    mov %ax, %ss
    leaq boot_stack_top(%rip), %rsp
    movl %ebx, %edi           # rdi = &hvm_start_info (EBX preserved since entry)
    xorl %ebp, %ebp
    callq isa_entry

halt64:
    cli
    hlt
    jmp halt64

.align 8
gdt_desc:
    .word gdt_end - gdt_start - 1
    .long gdt_start
.align 8
gdt_start:
    .quad 0
    .quad 0x00AF9A000000FFFF        # 0x08: 64-bit code (L=1, G=1)
    .quad 0x00CF92000000FFFF        # 0x10: data, base 0 limit 4G
    .quad 0x00CF9A000000FFFF        # 0x18: 32-bit code, base 0 limit 4G
gdt_end:

.section .bss.boot_stack, "aw", @nobits
.align 16
boot_stack_bottom:
    # 512 KiB: debug builds materialize every by-value move, and
    # Kernel::initialize shuttles the ~9 KiB EarlyFrameAllocator through
    # several stack frames (frame alloc -> closure -> Once static); 128 KiB
    # overflowed and corrupted memory below the stack.
    .space 512 * 1024
boot_stack_top:
