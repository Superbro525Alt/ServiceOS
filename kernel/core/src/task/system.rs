use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Once;

use crate::{
    capability::CapabilityRights,
    memory::{
        self,
        oom::{OomHooks, VictimCandidate},
    },
    object::{KernelObjectModel, KernelObjectRef, ObjectId},
    time::WakeEvent,
};

use super::{
    ExecutionState, ScheduleDecision, Scheduler, SchedulingContext, TaskId, TaskSystemSnapshot,
    ThreadDescriptor, ThreadId, ThreadMode,
};

pub struct TaskSystem {
    objects: &'static KernelObjectModel,
    bootstrap_task: TaskId,
    bootstrap_thread: KernelObjectRef,
    scheduler: Scheduler,
}

impl TaskSystem {
    fn new(objects: &'static KernelObjectModel) -> Self {
        let bootstrap_task = objects
            .bootstrap_task()
            .task()
            .expect("bootstrap task object")
            .id();
        let bootstrap_thread = objects.registry().create_thread(
            objects.bootstrap_task(),
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        objects
            .bootstrap_task()
            .task()
            .expect("bootstrap task object")
            .capability_space()
            .install(
                Arc::clone(&bootstrap_thread),
                CapabilityRights::thread(),
                Some(1),
            )
            .expect("bootstrap thread install must not exhaust the capability space");

        // OOM recovery hooks: the kernel frame allocator consults these when
        // an allocation fails, so victims are found among live tasks.
        memory::oom::register_oom_hooks(OomHooks {
            candidates: oom_candidate_scan,
            reclaim: oom_reclaim_victim,
        });

        Self {
            objects,
            bootstrap_task,
            scheduler: Scheduler::new(Arc::clone(&bootstrap_thread)),
            bootstrap_thread,
        }
    }

    pub fn snapshot(&self) -> TaskSystemSnapshot {
        TaskSystemSnapshot {
            bootstrap_task: self.bootstrap_task,
            bootstrap_thread: self.bootstrap_thread(),
            scheduler: self.scheduler.snapshot(),
        }
    }

    pub fn bootstrap_task(&self) -> TaskId {
        self.bootstrap_task
    }

    pub fn objects(&self) -> &'static KernelObjectModel {
        self.objects
    }

    pub fn bootstrap_thread_ref(&self) -> &KernelObjectRef {
        &self.bootstrap_thread
    }

    pub fn bootstrap_thread(&self) -> ThreadId {
        self.bootstrap_thread.thread().expect("thread object").id()
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn current_thread_object(&self) -> Option<KernelObjectRef> {
        let thread_id = self.scheduler.current_thread()?;
        self.objects.registry().lookup(ObjectId(thread_id.0))
    }

    pub fn current_task_object(&self) -> Option<KernelObjectRef> {
        let thread = self.current_thread_object()?;
        let owner = thread.thread()?.snapshot().owner;
        self.objects.registry().lookup(ObjectId(owner.0))
    }

    pub fn handle_time_wakeup(&self, event: WakeEvent) -> Option<ScheduleDecision> {
        self.scheduler.handle_time_wakeup(event)
    }

    pub fn handle_tick(&self) {
        self.scheduler.handle_tick();
    }

    pub fn consume_preemption(&self) -> bool {
        self.scheduler.consume_preemption()
    }

    pub fn notify_channel_ready(&self, endpoint: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_channel_ready(endpoint)
    }

    pub fn notify_packet_ready(&self, interface: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_packet_ready(interface)
    }

    pub fn notify_input_ready(&self, source: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_input_ready(source)
    }

    pub fn notify_object_ready(&self, object: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_object_ready(object)
    }
}

static TASK_SYSTEM: Once<TaskSystem> = Once::new();

pub fn initialize(objects: &'static KernelObjectModel) -> &'static TaskSystem {
    TASK_SYSTEM.call_once(|| TaskSystem::new(objects))
}

pub fn system() -> Option<&'static TaskSystem> {
    TASK_SYSTEM.get()
}

pub fn notify_channel_ready(endpoint: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_channel_ready(endpoint))
}

pub fn notify_packet_ready(interface: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_packet_ready(interface))
}

pub fn notify_input_ready(source: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_input_ready(source))
}

pub fn notify_object_ready(object: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_object_ready(object))
}

/// OOM candidate scan: one entry per task owning at least one tracked
/// thread, carrying its reclaimable mark and charged frame footprint.
fn oom_candidate_scan() -> Vec<VictimCandidate> {
    let Some(tasks) = system() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for owner in tasks.scheduler().tracked_thread_owners() {
        let Some(object) = tasks.objects().registry().lookup(ObjectId(owner.0)) else {
            continue;
        };
        let Some(task) = object.task() else {
            continue;
        };
        candidates.push(VictimCandidate {
            task: owner,
            name: None,
            footprint_frames: task.footprint_frames(),
            reclaimable: task.is_reclaimable(),
        });
    }
    candidates
}

/// OOM victim reclamation: fault-style termination with the distinct OOM
/// exit reason, deschedule the victim's threads, and hand its charged frames
/// back to the frame allocator before the retry allocation.
fn oom_reclaim_victim(candidate: VictimCandidate) {
    let Some(tasks) = system() else {
        return;
    };
    let Some(object) = tasks
        .objects()
        .registry()
        .lookup(ObjectId(candidate.task.0))
    else {
        return;
    };
    let Some(task) = object.task() else {
        return;
    };

    let frames = task.mark_oom_terminated();
    for thread_id in task.thread_ids() {
        if let Some(thread_object) = tasks.objects().registry().lookup(thread_id) {
            if let Some(thread) = thread_object.thread() {
                thread.transition_to(ExecutionState::Dying, None, None);
            }
        }
    }

    if frames.is_empty() {
        return;
    }
    if let Some(memory_manager) = memory::manager() {
        let mut allocator = memory_manager.frame_allocator().lock();
        for base in frames {
            allocator.free_4kib(base);
        }
    }
}
