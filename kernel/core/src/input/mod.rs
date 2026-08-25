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
                    source_id: 0,
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
            source_id: 0,
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

    // --- Multi-host enumeration, event tagging, stale-source handling ---

    use serviceos_abi::{
        InputDeviceInfo, InputEventKind, input_device_class, input_role_flag,
    };

    struct MultiHostBackend {
        devices: Mutex<Vec<InputDeviceInfo>>,
        events: Mutex<Vec<InputEventInfo>>,
    }

    fn instance(source_id: u32, class: u32, role_flags: u32) -> InputDeviceInfo {
        InputDeviceInfo {
            source_id,
            class,
            role_flags,
            present: 1,
        }
    }

    impl MultiHostBackend {
        fn standard() -> Self {
            Self {
                devices: Mutex::new(vec![
                    instance(1, input_device_class::KEYBOARD, 0),
                    instance(2, input_device_class::TABLET, input_role_flag::POSITIONAL_AUTHORITY),
                    instance(
                        3,
                        input_device_class::POINTER,
                        input_role_flag::SCROLL_ONLY,
                    ),
                ]),
                events: Mutex::new(vec![
                    InputEventInfo {
                        kind: 3,
                        code: 30,
                        value0: 0,
                        value1: 0,
                        source_id: 1,
                    },
                    InputEventInfo {
                        kind: 2,
                        code: 1,
                        value0: 1,
                        value1: 0,
                        source_id: 3,
                    },
                ]),
            }
        }
    }

    impl InputBackend for MultiHostBackend {
        fn info(&self) -> InputSourceInfo {
            let devices = self.devices.lock();
            InputSourceInfo {
                backend: InputSourceBackend::Unknown as u32,
                capabilities: input_capability::KEYBOARD | input_capability::POINTER,
                device_count: devices.iter().filter(|d| d.present != 0).count() as u32,
                pending_events: self.events.lock().len() as u32,
            }
        }

        fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
            let mut events = self.events.lock();
            let devices = self.devices.lock();
            let index = events
                .iter()
                .position(|event| {
                    devices
                        .iter()
                        .any(|d| d.source_id == event.source_id && d.present != 0)
                })
                .ok_or(InputSourceError::QueueEmpty)?;
            Ok(events.remove(index))
        }

        fn poll(&self) -> bool {
            false
        }

        fn enumerate_devices(&self) -> Vec<InputDeviceInfo> {
            self.devices.lock().clone()
        }

        fn set_device_present(&self, source_id: u32, present: bool) {
            let mut devices = self.devices.lock();
            for device in devices.iter_mut() {
                if device.source_id == source_id {
                    device.present = u32::from(present);
                }
            }
        }
    }

    #[test]
    fn enumeration_reports_each_instance_distinctly() {
        let backend = Arc::new(MultiHostBackend::standard());
        let source = InputSourceObject::new(backend);

        let devices = source.enumerate_devices();
        assert_eq!(devices.len(), 3);
        let ids: Vec<u32> = devices.iter().map(|d| d.source_id).collect();
        assert_eq!(ids, vec![1, 2, 3], "instances must be distinct");
        assert_eq!(devices[0].class, input_device_class::KEYBOARD);
        assert_eq!(devices[1].class, input_device_class::TABLET);
        assert_eq!(
            devices[1].role_flags,
            input_role_flag::POSITIONAL_AUTHORITY
        );
        assert_eq!(devices[2].class, input_device_class::POINTER);
        assert_eq!(devices[2].role_flags, input_role_flag::SCROLL_ONLY);
        assert!(devices.iter().all(|d| d.present == 1));
        assert_eq!(source.info().device_count, 3);
    }

    #[test]
    fn events_carry_source_id_through_receive_paths() {
        let backend = Arc::new(MultiHostBackend::standard());
        let source = InputSourceObject::new(backend);

        let first = source
            .try_receive_with_fallback()
            .expect("first host event");
        assert_eq!(first.source_id, 1);
        assert_eq!(first.kind, 3);
        let second = source.try_receive_with_fallback().expect("second host event");
        assert_eq!(second.source_id, 3, "secondary host tag must survive");
        assert_eq!(second.kind, InputEventKind::PointerButton as u32);
    }

    #[test]
    fn stale_source_marked_absent_is_ignored_without_wedging() {
        let backend = Arc::new(MultiHostBackend::standard());
        let source = InputSourceObject::new(backend);

        source.mark_device_absent(2);
        source.mark_device_absent(3);

        let devices = source.enumerate_devices();
        assert_eq!(devices[1].present, 0, "stale tablet reported absent");
        assert_eq!(devices[2].present, 0, "stale pointer reported absent");
        assert_eq!(devices[0].present, 1, "other hosts unaffected");
        assert_eq!(source.info().device_count, 1);

        // Queued events from absent hosts are skipped; pipeline keeps flowing.
        let only = source.try_receive_with_fallback().expect("keyboard event");
        assert_eq!(only.source_id, 1);
        assert!(matches!(
            source.try_receive_with_fallback(),
            Err(InputSourceError::QueueEmpty)
        ));

        source.mark_device_present(3);
        assert_eq!(source.enumerate_devices()[2].present, 1);
    }
}
