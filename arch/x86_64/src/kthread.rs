use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

use serviceos_kernel_core::task::KernelContext;

use crate::kernel_context::{init_kernel_thread_context, kernel_context_switch};
use crate::serial;

/// Stack size reserved for each kernel thread (16-aligned at spawn).
const KERNEL_THREAD_STACK_BYTES: usize = KERNEL_THREAD_STACK_WORDS * core::mem::size_of::<u64>();
const KERNEL_THREAD_STACK_WORDS: usize = 8192;

/// Number of per-CPU kernel-thread run queues. CPUs beyond this cap share
/// the last queue; steal-on-empty keeps every queue drainable from any CPU.
const RUN_QUEUE_CPUS: usize = 8;
/// Ping-pong demo workload: iterations per thread before exiting.
const DEMO_ROUNDS: u64 = 1000;

#[derive(Clone, Copy)]
struct ReadyEntry {
    tid: u32,
    /// `KernelContext` address as `usize` so entries stay `Send`.
    context: usize,
}

struct ThreadRecord {
    tid: u32,
    /// `KernelContext` address living at the base of the leaked stack.
    context: usize,
    /// Leaked stack allocation (kept alive for the thread's lifetime).
    _stack: usize,
}

static NEXT_THREAD_ID: AtomicU32 = AtomicU32::new(1);
static THREADS: Mutex<Vec<ThreadRecord>> = Mutex::new(Vec::new());
static READY: [Mutex<VecDeque<ReadyEntry>>; RUN_QUEUE_CPUS] =
    [const { Mutex::new(VecDeque::new()) }; RUN_QUEUE_CPUS];

/// Per-CPU runner slots: the context a kernel thread switches back into
/// (the parking executor frame or an AP idle frame), plus which thread is
/// currently running on the CPU. Only the owning CPU touches `ctx`.
struct RunnerSlots {
    contexts: [UnsafeCell<KernelContext>; RUN_QUEUE_CPUS],
    active: [AtomicUsize; RUN_QUEUE_CPUS],
    current: [AtomicU32; RUN_QUEUE_CPUS],
}

unsafe impl Sync for RunnerSlots {}

static RUNNERS: RunnerSlots = RunnerSlots {
    contexts: [const {
        UnsafeCell::new(KernelContext {
            rsp: 0,
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
        })
    }; RUN_QUEUE_CPUS],
    active: [const { AtomicUsize::new(0) }; RUN_QUEUE_CPUS],
    current: [const { AtomicU32::new(0) }; RUN_QUEUE_CPUS],
};

static SWITCHES: AtomicU64 = AtomicU64::new(0);
static DEMO_COUNTERS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static DEMO_SPAWNED: AtomicU32 = AtomicU32::new(0);
static DEMO_EXITED: AtomicU32 = AtomicU32::new(0);
static DEMO_SUMMARY_PRINTED: AtomicBool = AtomicBool::new(false);

/// Spawn a kernel thread running `entry(arg)` on a freshly allocated stack.
///
/// The thread is queued onto the least-loaded per-CPU run queue and is
/// picked up by whichever CPU pumps kernel threads next (the userspace
/// executor parking into them on the BSP, or an AP idle loop).
pub fn spawn(entry: extern "C" fn(u64) -> !, arg: u64) -> Option<u32> {
    let tid = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);

    let stack = unsafe {
        alloc::alloc::alloc(
            core::alloc::Layout::from_size_align(KERNEL_THREAD_STACK_BYTES, 16)
                .expect("kernel thread stack layout is valid"),
        )
    };
    if stack.is_null() {
        return None;
    }

    let context = unsafe { init_context_in(stack, entry, arg)? };

    THREADS.lock().push(ThreadRecord {
        tid,
        context: context as *mut KernelContext as usize,
        _stack: stack as usize,
    });

    push_ready_least_loaded(ReadyEntry {
        tid,
        context: context as *mut KernelContext as usize,
    });
    Some(tid)
}

/// Seed the entry stub frame at the top of a fresh stack and hand back the
/// stable context address (the stack is never moved).
unsafe fn init_context_in(
    stack: *mut u8,
    entry: extern "C" fn(u64) -> !,
    arg: u64,
) -> Option<&'static mut KernelContext> {
    let stack_top = (stack as usize + KERNEL_THREAD_STACK_BYTES) as u64;
    if stack_top % 16 != 0 {
        return None;
    }

    let context = stack as *mut KernelContext;
    unsafe {
        init_kernel_thread_context(&mut *context, entry, stack_top, arg);
    }
    Some(unsafe { &mut *context })
}

fn push_ready_least_loaded(entry: ReadyEntry) {
    let mut target = 0;
    let mut shortest = usize::MAX;
    for (index, queue) in READY.iter().enumerate() {
        let len = queue.lock().len();
        if len < shortest {
            shortest = len;
            target = index;
        }
    }
    READY[target].lock().push_back(entry);
}

fn push_ready_on(cpu: usize, entry: ReadyEntry) {
    READY[cpu % RUN_QUEUE_CPUS].lock().push_back(entry);
}

/// Pop from the calling CPU's queue first, stealing from the others
/// (lowest index first) when the local queue is empty.
fn pop_ready(cpu: usize) -> Option<ReadyEntry> {
    if let Some(entry) = READY[cpu % RUN_QUEUE_CPUS].lock().pop_front() {
        return Some(entry);
    }
    for queue in READY.iter() {
        if let Some(entry) = queue.lock().pop_front() {
            return Some(entry);
        }
    }
    None
}

/// Number of kernel threads currently waiting to run.
pub fn pending_count() -> usize {
    READY.iter().map(|queue| queue.lock().len()).sum()
}

fn context_of(tid: u32) -> Option<usize> {
    THREADS
        .lock()
        .iter()
        .find(|record| record.tid == tid)
        .map(|record| record.context)
}

/// Run every queued kernel thread on the calling CPU, parking into each via
/// a register-level context switch, until no queued work remains. Returns
/// the number of switch-ins performed.
pub fn pump_pending() -> usize {
    let mut switch_ins = 0;
    while let Some(entry) = pop_ready(0) {
        unsafe { run_one(0, entry) };
        switch_ins += 1;
    }
    switch_ins
}

/// AP idle loop: steal queued kernel threads, halting with interrupts
/// enabled between steals so an idle AP parks in `hlt` until the next
/// interrupt (or BSP-steal drains the queues).
pub fn ap_idle_loop(cpu: usize) -> ! {
    crate::cpu::enable_interrupts();
    loop {
        match pop_ready(cpu) {
            Some(entry) => {
                crate::cpu::disable_interrupts();
                crate::serial::write_args(format_args!(
                    "serviceos: smp: ap{} picked up kernel thread {}\n",
                    cpu,
                    RUNNERS.current[cpu % RUN_QUEUE_CPUS].load(Ordering::Relaxed),
                ));
                unsafe { run_one(cpu, entry) };
                crate::cpu::enable_interrupts();
            }
            None => crate::cpu::halt(),
        }
    }
}

/// # Safety
/// `entry.context` must be a live kernel thread context that no other CPU
/// is currently running, and interrupts must be disabled.
unsafe fn run_one(cpu: usize, entry: ReadyEntry) {
    unsafe {
        let slot = &RUNNERS.contexts[cpu % RUN_QUEUE_CPUS];
        RUNNERS.active[cpu % RUN_QUEUE_CPUS].store(slot.get() as usize, Ordering::Release);
        RUNNERS.current[cpu % RUN_QUEUE_CPUS].store(entry.tid, Ordering::Release);
        SWITCHES.fetch_add(1, Ordering::Relaxed);
        kernel_context_switch(&mut *slot.get(), &*(entry.context as *const KernelContext));
    }
}

/// Cooperative yield: requeue the calling kernel thread and switch back to
/// the runner that parked into it.
pub fn yield_now() {
    let cpu = current_cpu_index();
    let tid = RUNNERS.current[cpu].load(Ordering::Relaxed);
    let Some(context) = context_of(tid) else {
        return;
    };

    push_ready_on(cpu, ReadyEntry { tid, context });
    switch_to_runner(cpu, tid);
}

/// Terminate the calling kernel thread and return control to its runner.
pub fn exit_current() -> ! {
    let cpu = current_cpu_index();
    let tid = RUNNERS.current[cpu].load(Ordering::Relaxed);
    note_demo_exit();
    switch_to_runner(cpu, tid);
    unreachable!("kernel thread switched back into its runner must not return");
}

fn switch_to_runner(cpu: usize, tid: u32) {
    let runner = RUNNERS.active[cpu].load(Ordering::Acquire);
    let Some(context) = context_of(tid) else {
        return;
    };
    if runner == 0 {
        return;
    }

    SWITCHES.fetch_add(1, Ordering::Relaxed);
    unsafe {
        kernel_context_switch(
            &mut *(context as *mut KernelContext),
            &*(runner as *const KernelContext),
        );
    }
}

fn current_cpu_index() -> usize {
    // GS base is programmed per CPU; the BSP executor pumps queue 0 and APs
    // pass their own index, so fall back to the per-CPU id when available.
    // SAFETY: GS base points at this CPU's PerCpuData in every context that
    // can reach a kernel-thread switch.
    let cpu_id = unsafe { crate::per_cpu::current_cpu_data().cpu_id };
    cpu_id as usize % RUN_QUEUE_CPUS
}

fn note_demo_exit() {
    let spawned = DEMO_SPAWNED.load(Ordering::Relaxed);
    if spawned == 0 {
        return;
    }
    let exited = DEMO_EXITED.fetch_add(1, Ordering::Relaxed) + 1;
    if exited == spawned && !DEMO_SUMMARY_PRINTED.swap(true, Ordering::Relaxed) {
        serial::write_args(format_args!(
            "serviceos: kthread: ping-pong complete counters=({},{}) switches={}\n",
            DEMO_COUNTERS[0].load(Ordering::Relaxed),
            DEMO_COUNTERS[1].load(Ordering::Relaxed),
            SWITCHES.load(Ordering::Relaxed),
        ));
    }
}

extern "C" fn pingpong_entry(arg: u64) -> ! {
    let index = (arg as usize).min(DEMO_COUNTERS.len() - 1);
    for _ in 0..DEMO_ROUNDS {
        DEMO_COUNTERS[index].fetch_add(1, Ordering::Relaxed);
        yield_now();
    }
    exit_current();
}

/// Spawn the two-thread ping-pong demo used as the kernel-thread context
/// switch smoke test. Each thread increments its counter, yields across a
/// real register-level switch, and exits after `DEMO_ROUNDS` rounds.

pub fn spawn_pingpong_demo() {
    for index in 0..DEMO_COUNTERS.len() as u64 {
        if spawn(pingpong_entry, index).is_some() {
            DEMO_SPAWNED.fetch_add(1, Ordering::Relaxed);
        }
    }
}
