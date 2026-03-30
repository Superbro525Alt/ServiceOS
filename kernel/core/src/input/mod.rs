mod backend;
mod core;

pub use backend::{InputBackend, InputSourceError, InputSourceObject};
pub use core::{InputCore, initialize, manager};

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::sync::atomic::{AtomicUsize, Ordering};
    use alloc::sync::Arc;
    use serviceos_abi::{InputEventInfo, InputSourceBackend, InputSourceInfo};

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
}
