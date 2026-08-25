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

/// Classification of why a user task faulted. Carried additively inside the
/// user-fault exit-code word so existing single-word consumers keep working.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum FaultClass {
    Unknown,
    NullDeref,
    WildAddress,
    ExecNonExec,
    Permission,
}

impl FaultClass {
    pub const fn code(self) -> u64 {
        match self {
            FaultClass::Unknown => 0,
            FaultClass::NullDeref => 1,
            FaultClass::WildAddress => 2,
            FaultClass::ExecNonExec => 3,
            FaultClass::Permission => 4,
        }
    }

    pub const fn from_code(code: u64) -> Self {
        match code & 0xf {
            1 => FaultClass::NullDeref,
            2 => FaultClass::WildAddress,
            3 => FaultClass::ExecNonExec,
            4 => FaultClass::Permission,
            _ => FaultClass::Unknown,
        }
    }

    /// Operator-facing short name for console/log rendering.
    pub const fn name(self) -> &'static str {
        match self {
            FaultClass::Unknown => "unknown",
            FaultClass::NullDeref => "null-deref",
            FaultClass::WildAddress => "wild-addr",
            FaultClass::ExecNonExec => "exec-nonexec",
            FaultClass::Permission => "permission",
        }
    }
}

const PAGE_PROTECTION_VIOLATION: u64 = 1 << 0;
const PAGE_INSTRUCTION_FETCH: u64 = 1 << 4;

/// Classify a user page fault from its raw x86_64 inputs. Pure function so it
/// is host-testable and shared by the terminate and supervisor-upcall paths.
pub fn classify_page_fault(
    fault_address: u64,
    error_code: u64,
    instruction_pointer: u64,
) -> FaultClass {
    if error_code & PAGE_INSTRUCTION_FETCH != 0 {
        return FaultClass::ExecNonExec;
    }
    if error_code & PAGE_PROTECTION_VIOLATION != 0 {
        return FaultClass::Permission;
    }
    if fault_address < 0x1000 || fault_address == instruction_pointer {
        return FaultClass::NullDeref;
    }
    FaultClass::WildAddress
}

/// User-fault exit-code word layout (additive over the legacy encoding):
///   bits 63..48 tag `0xf100`
///   bits 47..16 low 32 bits of the faulting address (page faults) or of the
///               instruction pointer (every other exception)
///   bits 15..12 fault class (0 = unknown; legacy words decode as unknown)
///   bits 11..0  legacy detail selector (vector/error-code), byte-for-byte
///               identical to the pre-existing encoding
pub const USER_FAULT_EXIT_TAG: u64 = 0xf100_0000_0000_0000;

pub fn pack_user_fault_exit_code(detail: u64, class: FaultClass, address_or_ip: u64) -> u64 {
    USER_FAULT_EXIT_TAG
        | ((address_or_ip & 0xffff_ffff) << 16)
        | (class.code() << 12)
        | (detail & 0xfff)
}

pub fn is_user_fault_exit_code(exit_code: u64) -> bool {
    exit_code & 0xffff_0000_0000_0000 == USER_FAULT_EXIT_TAG
}

pub fn fault_class_from_exit_code(exit_code: u64) -> Option<FaultClass> {
    if !is_user_fault_exit_code(exit_code) {
        return None;
    }
    Some(FaultClass::from_code((exit_code >> 12) & 0xf))
}

pub fn user_fault_address_from_exit_code(exit_code: u64) -> Option<u64> {
    if !is_user_fault_exit_code(exit_code) {
        return None;
    }
    Some((exit_code >> 16) & 0xffff_ffff)
}

pub fn user_fault_detail_from_exit_code(exit_code: u64) -> Option<u64> {
    if !is_user_fault_exit_code(exit_code) {
        return None;
    }
    Some(exit_code & 0xfff)
}

/// Richer fault details captured at trap time, keyed by thread so a supervisor
/// or the exiting path can render them without widening any ABI struct.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserFaultRecord {
    pub class: FaultClass,
    pub fault_address: u64,
    pub instruction_pointer: u64,
}

static LAST_USER_FAULTS: Mutex<BTreeMap<ThreadId, UserFaultRecord>> = Mutex::new(BTreeMap::new());

pub fn record_user_fault(thread_id: ThreadId, record: UserFaultRecord) {
    LAST_USER_FAULTS.lock().insert(thread_id, record);
}

pub fn take_user_fault(thread_id: ThreadId) -> Option<UserFaultRecord> {
    LAST_USER_FAULTS.lock().remove(&thread_id)
}

pub fn last_user_fault(thread_id: ThreadId) -> Option<UserFaultRecord> {
    LAST_USER_FAULTS.lock().get(&thread_id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_null_derefs_and_wild_addresses() {
        assert_eq!(
            classify_page_fault(0x8, 0x0, 0x4000_1234),
            FaultClass::NullDeref
        );
        assert_eq!(
            classify_page_fault(0x7fff_ff00, 0x0, 0x4000_1234),
            FaultClass::WildAddress
        );
        assert_eq!(
            classify_page_fault(0xdead_beef_0000, 0x0, 0x4000_1234),
            FaultClass::WildAddress
        );
    }

    #[test]
    fn classifies_exec_of_nonexec_and_permission() {
        // Instruction-fetch bit dominates, including jumps to null.
        assert_eq!(
            classify_page_fault(0x0, PAGE_INSTRUCTION_FETCH, 0x0),
            FaultClass::ExecNonExec
        );
        assert_eq!(
            classify_page_fault(0x1000, PAGE_INSTRUCTION_FETCH, 0x1000),
            FaultClass::ExecNonExec
        );
        assert_eq!(
            classify_page_fault(0x2000, PAGE_PROTECTION_VIOLATION, 0x4000_1234),
            FaultClass::Permission
        );
        assert_eq!(
            classify_page_fault(
                0x2000,
                PAGE_PROTECTION_VIOLATION | PAGE_INSTRUCTION_FETCH,
                0x4000_1234
            ),
            FaultClass::ExecNonExec
        );
    }

    #[test]
    fn exit_code_packing_roundtrips_and_stays_legacy_compatible() {
        let packed =
            pack_user_fault_exit_code(0x100 | 0x2, FaultClass::NullDeref, 0x0000_badc_0000_0008);
        assert!(is_user_fault_exit_code(packed));
        assert_eq!(packed >> 48, 0xf100);
        assert_eq!(
            fault_class_from_exit_code(packed),
            Some(FaultClass::NullDeref)
        );
        assert_eq!(user_fault_detail_from_exit_code(packed), Some(0x102));
        assert_eq!(user_fault_address_from_exit_code(packed), Some(0x8));
        // A pre-existing legacy word (class bits zero, detail in the low
        // bits) still decodes byte-for-byte.
        let legacy = USER_FAULT_EXIT_TAG | 0x300 | 14;
        assert_eq!(
            fault_class_from_exit_code(legacy),
            Some(FaultClass::Unknown)
        );
        assert_eq!(user_fault_detail_from_exit_code(legacy), Some(0x30e));
        assert!(!is_user_fault_exit_code(0xf670));
    }

    #[test]
    fn fault_record_registry_roundtrips_per_thread() {
        let record = UserFaultRecord {
            class: FaultClass::Permission,
            fault_address: 0x5000,
            instruction_pointer: 0x4012_3456,
        };
        record_user_fault(ThreadId(11), record);
        assert_eq!(last_user_fault(ThreadId(11)), Some(record));
        assert_eq!(take_user_fault(ThreadId(11)), Some(record));
        assert_eq!(take_user_fault(ThreadId(11)), None);
    }
}
