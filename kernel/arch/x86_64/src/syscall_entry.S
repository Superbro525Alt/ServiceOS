.macro PUSH_GENERAL_REGS
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rdi
    push rsi
    push rbp
    push rdx
    push rcx
    push rbx
    push rax
.endm

.macro POP_GENERAL_REGS
    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rbp
    pop rsi
    pop rdi
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
.endm

.global serviceos_x86_64_syscall_entry
serviceos_x86_64_syscall_entry:
    PUSH_GENERAL_REGS
    mov rdi, rsp
    mov rax, rsp
    and rsp, -16
    sub rsp, 16
    mov [rsp], rax
    call serviceos_x86_64_handle_syscall
    mov rsp, [rsp]
    POP_GENERAL_REGS
    iretq
