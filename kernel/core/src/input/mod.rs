mod backend;
mod core;

pub use backend::{InputBackend, InputSourceError, InputSourceObject};
pub use core::{InputCore, initialize, manager};

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::sync::atomic::{AtomicUsize, Ordering};
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;
    use serviceos_abi::{InputEventInfo, InputSourceBackend, InputSourceInfo, input_capability};
    use spin::Mutex;

    struct FakeBackend {
        polls: AtomicUsize,
        receives: AtomicUsize,
    }

    impl InputBackend for FakeBackend {
        fn info(&self) -> InputSourceInfo {
            InputSourceInfo {
                backend: InputSourceBackend::Unknown as u32,
                capabilities: 0,
                device_count: 0,
                pending_events: 0,
            }
        }

        fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
            match self.receives.fetch_add(1, Ordering::SeqCst) {
                0 => Err(InputSourceError::QueueEmpty),
                _ => Ok(InputEventInfo {
                    kind: 1,
                    code: 2,
                    value0: 3,
                    value1: 4,
                }),
            }
        }

        fn poll(&self) -> bool {
            self.polls.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    #[test]
    fn blocking_receive_performs_one_shot_backend_poll() {
        let backend = Arc::new(FakeBackend {
            polls: AtomicUsize::new(0),
            receives: AtomicUsize::new(0),
        });
        let source = InputSourceObject::new(backend.clone());

        let event = source
            .receive()
            .expect("blocking receive should retry after poll");
        assert_eq!(event.kind, 1);
        assert_eq!(backend.polls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.receives.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn nonblocking_receive_performs_one_shot_backend_poll_fallback() {
        let backend = Arc::new(FakeBackend {
            polls: AtomicUsize::new(0),
            receives: AtomicUsize::new(0),
        });
        let source = InputSourceObject::new(backend.clone());

        let event = source
            .try_receive_with_fallback()
            .expect("nonblocking receive should retry after poll");
        assert_eq!(event.kind, 1);
        assert_eq!(backend.polls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.receives.load(Ordering::SeqCst), 2);
    }

    struct ScriptedBackend {
        receive_results: Mutex<Vec<Result<InputEventInfo, InputSourceError>>>,
        polls: AtomicUsize,
        queue_depth: AtomicUsize,
    }

    impl ScriptedBackend {
        fn new(receive_results: Vec<Result<InputEventInfo, InputSourceError>>) -> Self {
            let queued = receive_results.iter().filter(|r| r.is_ok()).count();
            Self {
                receive_results: Mutex::new(receive_results),
                polls: AtomicUsize::new(0),
                queue_depth: AtomicUsize::new(queued),
            }
        }
    }

    const RACED_SOURCE_OBJECT_ID: u64 = 77;
    const PLAIN_SOURCE_OBJECT_ID: u64 = 78;
    const PENDING_SOURCE_OBJECT_ID: u64 = 79;

    fn push_event(kind: u32) -> Result<InputEventInfo, InputSourceError> {
        Ok(InputEventInfo {
            kind,
            code: 0,
            value0: 0,
            value1: 0,
        })
    }

    impl InputBackend for ScriptedBackend {
        fn info(&self) -> InputSourceInfo {
            InputSourceInfo {
                backend: InputSourceBackend::Unknown as u32,
                capabilities: input_capability::KEYBOARD,
                device_count: 1,
                pending_events: self.queue_depth.load(Ordering::SeqCst) as u32,
            }
        }

        fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
            let mut results = self.receive_results.lock();
            if results.is_empty() {
                return Err(InputSourceError::QueueEmpty);
            }
            let result = results.remove(0);
            if result.is_ok() {
                let _ = self.queue_depth.fetch_sub(1, Ordering::SeqCst);
            }
            result
        }

        fn poll(&self) -> bool {
            self.polls.fetch_add(1, Ordering::SeqCst);
            false
        }
    }

    #[test]
    fn blocking_receive_recovers_from_raced_wakeup_latch() {
        initialize();
        let backend = Arc::new(ScriptedBackend::new(vec![
            Err(InputSourceError::QueueEmpty),
            Err(InputSourceError::QueueEmpty),
            push_event(1),
        ]));
        let source = InputSourceObject::new(backend.clone());
        let core = manager().expect("input core initialized");
        let registered: Arc<dyn InputBackend> = backend.clone();
        assert!(core.register_test_source_for_latch(RACED_SOURCE_OBJECT_ID, registered));

        let latched: Arc<dyn InputBackend> = backend.clone();
        core.latch_wakeup(&latched);
        let event = source
            .receive()
            .expect("latched wakeup must trigger one re-drain before blocking");
        assert_eq!(event.kind, 1);
        assert!(
            !core.latch_peek(&latched),
            "latch must be consumed by the recovery attempt"
        );
    }

    #[test]
    fn blocking_receive_reports_empty_when_no_wakeup_raced() {
        initialize();
        let backend = Arc::new(ScriptedBackend::new(vec![Err(
            InputSourceError::QueueEmpty,
        )]));
        let source = InputSourceObject::new(backend.clone());
        let core = manager().expect("input core initialized");
        let registered: Arc<dyn InputBackend> = backend.clone();
        assert!(core.register_test_source_for_latch(PLAIN_SOURCE_OBJECT_ID, registered));

        assert!(matches!(
            source.receive(),
            Err(InputSourceError::QueueEmpty)
        ));
    }

    #[test]
    fn nonblocking_drain_collects_burst_until_empty() {
        let backend = Arc::new(ScriptedBackend::new(vec![
            push_event(1),
            push_event(2),
            push_event(3),
            Err(InputSourceError::QueueEmpty),
        ]));
        let source = InputSourceObject::new(backend);

        let mut drained = Vec::new();
        loop {
            match source.try_receive_with_fallback() {
                Ok(event) => drained.push(event.kind),
                Err(InputSourceError::QueueEmpty) => break,
                Err(error) => panic!("unexpected error: {error:?}"),
            }
        }
        assert_eq!(drained, vec![1, 2, 3]);
    }

    #[test]
    fn poll_ready_notifies_while_events_remain_pending() {
        initialize();
        let backend = Arc::new(ScriptedBackend::new(vec![push_event(9), push_event(9)]));
        let core = manager().expect("input core initialized");
        let registered: Arc<dyn InputBackend> = backend.clone();
        assert!(core.register_test_source_for_latch(PENDING_SOURCE_OBJECT_ID, registered));

        let mut notified = Vec::new();
        let polled: Arc<dyn InputBackend> = backend.clone();
        let latched: Arc<dyn InputBackend> = backend.clone();
        core.poll_ready_for_test(polled, |id| {
            notified.push(id);
        });
        assert_eq!(
            notified,
            vec![PENDING_SOURCE_OBJECT_ID],
            "pending events must notify even without a 0->nonempty edge"
        );
        assert!(core.latch_peek(&latched), "notification must latch wakeup");
    }
}
