.macro PUSH_GENERAL_REGS_SYSCALL
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

.macro POP_GENERAL_REGS_SYSCALL
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

.global serviceos_x86_64_syscall_fast_entry
serviceos_x86_64_syscall_fast_entry:
    # GS base permanently holds this CPU's PerCpuData: initialize_per_cpu_data
    # programs it once at boot and nothing swaps it, so no swapgs is needed on
    # entry or exit. Both directions below stay balanced (zero swaps).
    mov gs:[0x08], rsp          # Save user RSP in per-CPU data (user_rsp)
    mov rsp, gs:[0x00]          # Load kernel RSP from per-CPU data
    
    # Save user context
    PUSH_GENERAL_REGS_SYSCALL
    
    # Align stack to 16 bytes
    mov rax, rsp
    and rsp, -16
    sub rsp, 40
    mov [rsp + 32], rax
    
    # Call Rust handler
    call serviceos_x86_64_handle_syscall_fast
    
    # Check if we need to switch to kernel
    test rax, rax
    jnz serviceos_x86_64_syscall_fast_exit_to_kernel
    
    # Return to user via SYSRET
    mov rsp, [rsp + 32]
    POP_GENERAL_REGS_SYSCALL
    sysretq

serviceos_x86_64_syscall_fast_exit_to_kernel:
    # Switch to kernel stack for context switch
    mov rsp, [rip + serviceos_x86_64_user_return_stack]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rsi
    pop rdi
    pop rbp
    pop rbx
    ret
