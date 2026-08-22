.macro PUSH_TIMER_GENERAL_REGS
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

.macro POP_TIMER_GENERAL_REGS
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

# Raw timer IRQ frame layout produced by PUSH_TIMER_GENERAL_REGS followed by the
# CPU's 5-qword interrupt frame. The general-purpose register order matches
# SavedUserContext exactly, so Rust can overlay its context struct on the low
# 120 bytes and copy the IRET tail into the remaining fields.
.global serviceos_x86_64_timer_irq_entry
serviceos_x86_64_timer_irq_entry:
    PUSH_TIMER_GENERAL_REGS
    mov rax, rsp
    mov rcx, rax
    and rsp, -16
    sub rsp, 40
    mov [rsp + 32], rax
    mov rdi, rax
    call serviceos_x86_64_handle_timer_irq
    mov rsp, [rsp + 32]
    test rax, rax
    jnz serviceos_x86_64_timer_irq_preempted
    POP_TIMER_GENERAL_REGS
    iretq

serviceos_x86_64_timer_irq_preempted:
    # Abandon the IRQ frame and resume the interrupted kernel continuation of
    # run_thread(); the saved kernel stack lives in user_return_stack.
    jmp serviceos_x86_64_syscall_exit_to_kernel
