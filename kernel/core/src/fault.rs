use alloc::collections::BTreeMap;
use spin::Mutex;

use crate::{object::ObjectId, task::ThreadId};

/// Fault types that can be handled by user-space handlers
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum FaultType {
    /// Invalid opcode exception
    InvalidOpcode,
    /// Page fault
    PageFault,
    /// General protection fault
    GeneralProtection,
    /// Breakpoint exception
    Breakpoint,
    /// Other exceptions
    Other(u8),
}

/// Fault handler information
#[derive(Clone, Copy, Debug)]
pub struct FaultHandler {
    /// The thread that registered this handler
    pub thread_id: ThreadId,
    /// The endpoint to notify when a fault occurs
    pub endpoint: ObjectId,
    /// The fault type this handler is registered for
    pub fault_type: FaultType,
}

/// Fault handler registry
pub struct FaultHandlerRegistry {
    handlers: BTreeMap<FaultType, FaultHandler>,
}

impl FaultHandlerRegistry {
    /// Create a new fault handler registry
    pub const fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
        }
    }

    /// Register a fault handler for a specific fault type
    pub fn register(
        &mut self,
        fault_type: FaultType,
        handler: FaultHandler,
    ) -> Result<(), FaultRegistrationError> {
        if self.handlers.contains_key(&fault_type) {
            return Err(FaultRegistrationError::AlreadyRegistered);
        }
        self.handlers.insert(fault_type, handler);
        Ok(())
    }

    /// Unregister a fault handler for a specific fault type
    pub fn unregister(&mut self, fault_type: &FaultType) -> Result<(), FaultRegistrationError> {
        if self.handlers.remove(fault_type).is_none() {
            return Err(FaultRegistrationError::NotRegistered);
        }
        Ok(())
    }

    /// Look up a fault handler for a specific fault type
    pub fn lookup(&self, fault_type: &FaultType) -> Option<&FaultHandler> {
        self.handlers.get(fault_type)
    }

    /// Check if a fault handler is registered for a specific fault type
    pub fn is_registered(&self, fault_type: &FaultType) -> bool {
        self.handlers.contains_key(fault_type)
    }
}

/// Errors that can occur during fault handler registration
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultRegistrationError {
    /// A handler is already registered for this fault type
    AlreadyRegistered,
    /// No handler is registered for this fault type
    NotRegistered,
}

/// Global fault handler registry
static FAULT_HANDLERS: Mutex<FaultHandlerRegistry> = Mutex::new(FaultHandlerRegistry::new());

/// Register a fault handler for a specific fault type
pub fn register_fault_handler(
    fault_type: FaultType,
    thread_id: ThreadId,
    endpoint: ObjectId,
) -> Result<(), FaultRegistrationError> {
    FAULT_HANDLERS.lock().register(
        fault_type,
        FaultHandler {
            thread_id,
            endpoint,
            fault_type,
        },
    )
}

/// Unregister a fault handler for a specific fault type
pub fn unregister_fault_handler(fault_type: &FaultType) -> Result<(), FaultRegistrationError> {
    FAULT_HANDLERS.lock().unregister(fault_type)
}

/// Look up a fault handler for a specific fault type
pub fn lookup_fault_handler(fault_type: &FaultType) -> Option<FaultHandler> {
    FAULT_HANDLERS.lock().lookup(fault_type).copied()
}

/// Check if a fault handler is registered for a specific fault type
pub fn has_fault_handler(fault_type: &FaultType) -> bool {
    FAULT_HANDLERS.lock().is_registered(fault_type)
}

/// Convert an exception detail to a fault type
pub fn fault_type_for_exception(detail: &crate::interrupts::ExceptionDetail) -> FaultType {
    match detail {
        crate::interrupts::ExceptionDetail::InvalidOpcode => FaultType::InvalidOpcode,
        crate::interrupts::ExceptionDetail::PageFault { .. } => FaultType::PageFault,
        crate::interrupts::ExceptionDetail::GeneralProtection { .. } => {
            FaultType::GeneralProtection
        }
        crate::interrupts::ExceptionDetail::Breakpoint => FaultType::Breakpoint,
        crate::interrupts::ExceptionDetail::Unknown { vector, .. } => FaultType::Other(vector.0),
        crate::interrupts::ExceptionDetail::DoubleFault { .. } => FaultType::Other(8),
    }
}
