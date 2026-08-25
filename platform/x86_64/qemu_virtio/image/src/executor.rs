use serviceos_kernel_arch_x86_64::{cpu, interrupts, kthread, user};
use serviceos_kernel_core::{
    Kernel,
    object::ObjectId,
    task::{ExecutionState, ThreadMode},
    user::{self as kernel_user, TaskExitStatus},
};
use serviceos_platform_qemu_virtio::input;

use crate::bootstrap::BootstrapError;

pub(crate) fn run_userspace_executor(
    kernel: &Kernel<'_>,
    root_task: serviceos_kernel_core::task::TaskId,
) -> Result<(), BootstrapError> {
    loop {
        while let Some(event) = interrupts::poll_wakeup() {
            let _ = kernel.tasks().handle_time_wakeup(event);
        }

        // Drain input devices on every executor pass, independent of device
        // interrupts. VirtIO-input uses VIRTIO_RING_F_EVENT_IDX suppression:
        // after the first completion the device re-raises its INTx line only
        // once the driver republishes `used_event`, which only happens when
        // the driver pops - so an IRQ-only drain starves after one event
        // (mouse and tablet even share one level-triggered line here).
        // Polling here keeps host mouse/keyboard flowing under -display gtk;
        // the executor revisits this loop at least once per timer tick while
        // waiters are parked, bounding input latency to one tick.
        input::poll_ready_sources();

        let scheduler = kernel.tasks().scheduler();
        let snapshot = scheduler.snapshot();

        // With no runnable user thread, park the executor into queued kernel
        // threads (register-level context switches) until they drain.
        if snapshot.runnable_threads == 0 && kthread::pending_count() > 0 {
            kthread::pump_pending();
            continue;
        }

        let current = snapshot.current;
        let root_status = kernel_user::runtime()
            .and_then(|runtime| runtime.task_exit_status(root_task))
            .unwrap_or(TaskExitStatus::Running);

        if matches!(root_status, TaskExitStatus::Exited { .. }) && snapshot.runnable_threads == 0 {
            return Ok(());
        }

        let Some(thread_id) = current else {
            return Ok(());
        };

        if thread_id == kernel.tasks().bootstrap_thread() {
            if snapshot.runnable_threads > 0 {
                let _ = scheduler.yield_current()?;
                continue;
            }
            if snapshot.blocked_threads > 0 {
                park_until_interrupt();
                continue;
            }
            return Ok(());
        }

        if kernel.tasks().consume_preemption() || snapshot.preemption_pending {
            let _ = scheduler.preempt_current_if_needed()?;
            continue;
        }

        let Some(thread_object) = kernel.objects().registry().lookup(ObjectId(thread_id.0)) else {
            return Err(BootstrapError::MissingRootThread);
        };
        let Some(thread_state) = thread_object.thread().map(|thread| thread.snapshot()) else {
            return Err(BootstrapError::MissingRootThread);
        };

        if thread_state.mode == ThreadMode::User
            && thread_state.execution_state == ExecutionState::Running
        {
            user::run_thread(thread_id)?;
        } else if snapshot.runnable_threads == 0 && kthread::pending_count() == 0 {
            park_until_interrupt();
        } else {
            let _ = scheduler.yield_current()?;
        }
    }
}

/// Sleep until the next interrupt (timer tick or device IRQ) makes waiters
/// runnable. The enable/halt/disable window keeps the executor's interrupt
/// invariant (IF=0 outside user execution) intact across the park.
fn park_until_interrupt() {
    cpu::enable_interrupts();
    cpu::halt();
    cpu::disable_interrupts();
}
