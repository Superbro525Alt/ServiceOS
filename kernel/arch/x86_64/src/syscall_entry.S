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
    mov rax, rsp
    mov rcx, rax
    and rsp, -16
    sub rsp, 40
    mov [rsp + 32], rax
    call serviceos_x86_64_handle_syscall
    test rax, rax
    jnz serviceos_x86_64_syscall_exit_to_kernel
    mov rsp, [rsp + 32]
    POP_GENERAL_REGS
    iretq

serviceos_x86_64_syscall_exit_to_kernel:
    mov rsp, [rip + serviceos_x86_64_user_return_stack]
    ret
