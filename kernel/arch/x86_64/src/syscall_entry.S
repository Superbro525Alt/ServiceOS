.macro PUSH_GENERAL_REGS
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
.endm

.macro POP_GENERAL_REGS
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax
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
