use core::arch::global_asm;

use crate::{cpu, interrupts, serial};
use serviceos_kernel_core::memory::PhysicalAddress;
use spin::Mutex;

global_asm!(
    r#"
.global serviceos_x86_64_enter_user
serviceos_x86_64_enter_user:
    mov [rip + serviceos_x86_64_user_return_stack], rsp
    mov rax, [rsp + 0x28]
    push r9
    push rdx
    push rax
    push r8
    push rcx
    iretq
"#
);

unsafe extern "C" {
    fn serviceos_x86_64_enter_user(
        entry_point: u64,
        user_stack_pointer: u64,
        user_code_segment: u64,
        user_stack_segment: u64,
        rflags: u64,
    );
}

#[unsafe(no_mangle)]
static mut serviceos_x86_64_user_return_stack: u64 = 0;

static USER_EXIT_STATUS: Mutex<Option<u64>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserExitStatus {
    pub code: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserLaunchError {
    MissingExitStatus,
}

pub fn run_user_program(
    page_table_root: PhysicalAddress,
    entry_point: u64,
    user_stack_pointer: u64,
) -> Result<UserExitStatus, UserLaunchError> {
    *USER_EXIT_STATUS.lock() = None;
    let kernel_page_table_root = cpu::current_page_table_root();

    unsafe {
        cpu::load_page_table_root(page_table_root);
        serviceos_x86_64_enter_user(
            entry_point,
            user_stack_pointer,
            interrupts::user_code_selector().0 as u64,
            interrupts::user_data_selector().0 as u64,
            0x202,
        );
        cpu::load_page_table_root(kernel_page_table_root);
        serial::write_line("serviceos: userspace: returned to kernel from ring 3");
    }

    USER_EXIT_STATUS
        .lock()
        .take()
        .map(|code| UserExitStatus { code })
        .ok_or(UserLaunchError::MissingExitStatus)
}

pub fn record_user_exit(status: u64) {
    *USER_EXIT_STATUS.lock() = Some(status);
}
