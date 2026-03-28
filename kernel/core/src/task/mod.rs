mod objects;
mod scheduler;
mod system;
mod types;

pub use objects::{TaskObject, ThreadObject};
pub use scheduler::Scheduler;
pub use system::{
    TaskSystem, initialize, notify_channel_ready, notify_input_ready, notify_packet_ready, system,
};
pub use types::{
    AddressSpaceId, ExecutionState, ProcessId, ScheduleDecision, ScheduleTrigger, SchedulerError,
    SchedulerSnapshot, SchedulingContext, TaskCreationError, TaskDescriptor, TaskId, TaskManager,
    TaskRole, TaskStateView, TaskSystemSnapshot, ThreadDescriptor, ThreadId, ThreadMode,
    ThreadStateView, ThreadWakeReason, WaitTarget,
};

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::{
        bootstrap::{BootContext, BootMemoryRegion, BootMemoryRegionKind},
        memory::PhysicalAddress,
        object::{KernelObjectRef, ObjectId, ObjectRegistry},
        time::{self, MonotonicInstant, TimerSourceInfo, WakeEvent, WakeReason},
    };

    fn init_test_time() {
        let _ = time::initialize(TimerSourceInfo { tick_hz: 100 });
    }

    fn test_registry() -> (ObjectRegistry, KernelObjectRef, KernelObjectRef) {
        let registry = ObjectRegistry::new();
        let task = registry.create_bootstrap_root_task();
        let thread = registry.create_thread(
            &task,
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        (registry, task, thread)
    }

    #[test]
    fn scheduler_wakes_blocked_receiver() {
        let (registry, task, bootstrap_thread) = test_registry();
        let scheduler = Scheduler::new(bootstrap_thread);
        let worker = registry.create_thread(
            &task,
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        let worker_id = scheduler
            .register_thread(Arc::clone(&worker))
            .expect("worker should register");
        let endpoint = ObjectId(42);

        scheduler
            .make_runnable(worker_id, ThreadWakeReason::Explicit)
            .expect("worker should become runnable");
        let switch = scheduler.yield_current().expect("yield should succeed");
        assert_eq!(switch.next, Some(worker_id));

        let block = scheduler
            .block_current_on_receive(endpoint)
            .expect("blocking receive should succeed");
        assert_eq!(block.previous, Some(worker_id));
        assert_eq!(block.next, Some(ThreadId(2)));

        let wake = scheduler
            .notify_channel_ready(endpoint)
            .expect("receiver wake should produce a decision");
        assert_eq!(wake.trigger, ScheduleTrigger::IpcWake);

        let worker_state = worker.thread().expect("thread object").snapshot();
        assert_eq!(worker_state.execution_state, ExecutionState::Runnable);
        assert_eq!(
            worker_state.last_wake_reason,
            Some(ThreadWakeReason::ChannelMessage)
        );
    }

    #[test]
    fn scheduler_wakes_timer_blocked_thread() {
        init_test_time();

        let (registry, task, bootstrap_thread) = test_registry();
        let scheduler = Scheduler::new(bootstrap_thread);
        let worker = registry.create_thread(
            &task,
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        let worker_id = scheduler
            .register_thread(Arc::clone(&worker))
            .expect("worker should register");

        scheduler
            .make_runnable(worker_id, ThreadWakeReason::Explicit)
            .expect("worker should become runnable");
        let _ = scheduler.yield_current().expect("yield should succeed");

        let (token, block) = scheduler
            .block_current_until(MonotonicInstant(5))
            .expect("timer block should succeed");
        assert_eq!(block.previous, Some(worker_id));
        assert_eq!(block.next, Some(ThreadId(2)));

        let wake = scheduler
            .handle_time_wakeup(WakeEvent {
                token,
                reason: WakeReason::DeadlineExpired,
            })
            .expect("time wake should produce a decision");
        assert_eq!(wake.trigger, ScheduleTrigger::TimeWake);

        let worker_state = worker.thread().expect("thread object").snapshot();
        assert_eq!(worker_state.execution_state, ExecutionState::Runnable);
        assert_eq!(
            worker_state.last_wake_reason,
            Some(ThreadWakeReason::TimerExpired)
        );
    }

    #[test]
    fn block_current_until_reports_wake_token_exhaustion() {
        init_test_time();

        let (_registry, _task, bootstrap_thread) = test_registry();
        let scheduler = Scheduler::new(bootstrap_thread);
        scheduler.set_next_wake_token_for_test(u64::MAX);

        assert_eq!(
            scheduler.block_current_until(MonotonicInstant(1)),
            Err(SchedulerError::WakeTokenExhausted)
        );
    }

    #[test]
    fn boot_context_counts_memory_kinds() {
        let regions = [
            BootMemoryRegion {
                start: PhysicalAddress::new(0x1000),
                end: PhysicalAddress::new(0x3000),
                kind: BootMemoryRegionKind::Usable,
            },
            BootMemoryRegion {
                start: PhysicalAddress::new(0x3000),
                end: PhysicalAddress::new(0x4000),
                kind: BootMemoryRegionKind::BootServicesReclaimable,
            },
        ];
        let context = BootContext {
            memory_regions: &regions,
            memory_map_available: true,
            memory_map_truncated: false,
            physical_memory_offset: None,
            rsdp_address: None,
            framebuffer: None,
            boot_store: None,
        };

        assert_eq!(context.usable_memory_region_count(), 1);
        assert_eq!(context.boot_services_reclaimable_region_count(), 1);
    }
}
