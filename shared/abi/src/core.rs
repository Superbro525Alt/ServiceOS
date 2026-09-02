pub type Handle = u32;

pub const INVALID_HANDLE: Handle = 0;
pub const IPC_MAX_WORDS: usize = 16;
pub const IPC_MAX_HANDLES: usize = 8;
pub const IPC_FLAG_NONBLOCK: u32 = 1 << 0;
pub const IPC_FLAG_RECEIVE_TIMEOUT: u32 = 1 << 1;
pub const OBJECT_WAIT_FLAG_NONBLOCK: u32 = 1 << 0;
pub const PIPE_FLAG_NONBLOCK: u32 = 1 << 0;

pub mod memory_map_flags {
    pub const WRITABLE: u32 = 1 << 0;
    pub const FIXED: u32 = 1 << 1;
}

pub mod object_state_flags {
    pub const READY: u32 = 1 << 0;
    pub const SIGNALED: u32 = 1 << 1;
    pub const ARMED: u32 = 1 << 2;
    pub const WRITABLE: u32 = 1 << 3;
    pub const RUNNING: u32 = 1 << 4;
    pub const EXITED: u32 = 1 << 5;
    pub const FAULTED: u32 = 1 << 6;
}

pub mod rights {
    pub const NONE: u64 = 0;
    pub const READ: u64 = 1 << 0;
    pub const WRITE: u64 = 1 << 1;
    pub const MAP: u64 = 1 << 2;
    pub const SIGNAL: u64 = 1 << 3;
    pub const WAIT: u64 = 1 << 4;
    pub const SEND: u64 = 1 << 5;
    pub const RECEIVE: u64 = 1 << 6;
    pub const DUPLICATE: u64 = 1 << 7;
    pub const TRANSFER: u64 = 1 << 8;
    pub const MANAGE: u64 = 1 << 9;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallNumber {
    AbiVersion = 0,
    MonotonicNow = 1,
    ThreadExit = 2,
    YieldCurrent = 3,
    DebugLogWrite = 4,
    ChannelCreate = 5,
    ChannelSend = 6,
    ChannelReceive = 7,
    HandleDuplicate = 8,
    HandleClose = 9,
    ServiceSpawn = 10,
    TaskStatus = 11,
    MemoryRead = 12,
    DebugConsoleRead = 13,
    DebugConsoleWrite = 14,
    PacketInterfaceInfo = 15,
    PacketInterfaceReceive = 16,
    PacketInterfaceTransmit = 17,
    DisplayOutputInfo = 18,
    DisplayOutputPresent = 19,
    InputSourceInfo = 20,
    InputSourceReceive = 21,
    MemoryCreate = 22,
    MemoryWrite = 23,
    AudioEndpointInfo = 24,
    AudioEndpointPlayTone = 25,
    AudioEndpointStop = 26,
    MemoryMap = 27,
    TaskSpawnImage = 28,
    BlockDeviceInfo = 29,
    BlockDeviceRead = 30,
    BlockDeviceWrite = 31,
    MemoryInfo = 32,
    MemoryMapRange = 33,
    EventCreate = 34,
    EventSignal = 35,
    EventReset = 36,
    ObjectInfo = 37,
    ObjectWait = 38,
    KernelEventQueryInfo = 39,
    KernelEventQueryRecord = 40,
    DisplayOutputPresentDamage = 41,
    MemoryUnmap = 42,
    MemoryProtect = 43,
    MemoryQuery = 44,
    FaultHandlerRegister = 45,
    FaultHandlerUnregister = 46,
    TaskLoadedLibraries = 47,
    AudioEndpointPcmWrite = 48,
    PipeCreate = 49,
    PipeRead = 50,
    PipeWrite = 51,
    PacketInterfaceRingSetup = 52,
    PacketInterfaceTxRingSetup = 53,
    PacketInterfaceTxRingFlush = 54,
    /// Fill a caller buffer with kernel-DRBG bytes: args are (buffer
    /// pointer, max length); returns the number of bytes written. Errors
    /// with NotInitialized when the kernel RNG subsystem has not been
    /// seeded (callers keep their documented entropy substitutes).
    RngRequest = 55,
    /// Runtime-load an additional ELF64 `ET_DYN` shared object into the
    /// calling task's address space: args are (memory-object handle holding
    /// the library bytes, flags word, reserved, reserved). Returns a
    /// library handle usable with [`SyscallNumber::TaskSymbolLookup`] and
    /// reported by [`SyscallNumber::TaskLoadedLibraries`]; the mapping base
    /// rides the library record. Unknown flag bits are rejected loudly.
    TaskLoadLibrary = 56,
    /// Resolve a symbol name against the calling task's load-scoped symbol
    /// table: args are (library handle from TaskLoadLibrary, name pointer,
    /// name byte length). Returns the resolved absolute address, or
    /// NotFound when no definition exists for the name.
    TaskSymbolLookup = 57,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallErrorCode {
    Ok = 0,
    Unsupported = 1,
    InvalidCall = 2,
    PermissionDenied = 3,
    NotInitialized = 4,
    InvalidArgument = 5,
    BufferTooSmall = 6,
    QueueEmpty = 7,
    NotFound = 8,
    Busy = 9,
    CapacityExceeded = 10,
    BrokenPipe = 11,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandlePair {
    pub first: Handle,
    pub second: Handle,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawMessage {
    pub tag: u32,
    pub word_count: u32,
    pub handle_count: u32,
    pub flags: u32,
    pub words: [u64; IPC_MAX_WORDS],
    pub handles: [Handle; IPC_MAX_HANDLES],
    pub handle_rights: [u64; IPC_MAX_HANDLES],
}

impl RawMessage {
    pub const fn empty(tag: u32) -> Self {
        Self {
            tag,
            word_count: 0,
            handle_count: 0,
            flags: 0,
            words: [0; IPC_MAX_WORDS],
            handles: [INVALID_HANDLE; IPC_MAX_HANDLES],
            handle_rights: [0; IPC_MAX_HANDLES],
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlTag {
    Startup = 1,
    Register = 2,
    LookupRequest = 3,
    LookupReply = 4,
    Lifecycle = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookupStatus {
    Ok = 0,
    Denied = 1,
    Unavailable = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    Starting = 1,
    Ready = 2,
    Failed = 3,
    Restarting = 4,
    Stopped = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskStateCode {
    Running = 1,
    Exited = 2,
    Faulted = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskStatus {
    pub state: TaskStateCode,
    pub exit_code: u64,
}

/// One library image mapped into a task's address space by the loader.
/// Spawn-time companions carry their BootStore `image_id` and
/// `library_handle = 0`; runtime-loaded libraries (extended flat-image
/// headers or `TaskLoadLibrary`) carry `image_id = 0` and the handle
/// returned by `TaskLoadLibrary`. Returned by
/// [`SyscallNumber::TaskLoadedLibraries`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskLoadedLibrary {
    pub image_id: u32,
    pub library_handle: u32,
    pub base: u64,
    pub mapped_bytes: u64,
}

/// `TaskLoadLibrary` flags word. v0 accepts only the zero word; any other
/// bit is rejected so future policy cannot be silently ignored.
pub mod task_load_library_flags {
    pub const VALID_MASK: u64 = 0;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryObjectInfo {
    pub size_bytes: usize,
    pub page_count: usize,
    pub writable: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryMapRequest {
    pub offset_bytes: usize,
    pub length_bytes: usize,
    pub address_hint: u64,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKindCode {
    Task = 1,
    Thread = 2,
    ChannelEndpoint = 3,
    Event = 4,
    Timer = 5,
    MemoryObject = 6,
    BootstrapCapability = 7,
    PacketInterface = 8,
    DisplayOutput = 9,
    InputSource = 10,
    AudioEndpoint = 11,
    BlockDevice = 12,
    Pipe = 13,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectInfo {
    pub object_id: u64,
    pub kind: ObjectKindCode,
    pub state_flags: u32,
    pub reserved: u32,
    pub detail0: u64,
    pub detail1: u64,
    pub detail2: u64,
    pub detail3: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelEventKind {
    Trap = 1,
    Pressure = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelEventRecord {
    pub sequence: u64,
    pub kind: KernelEventKind,
    pub reserved: u32,
    pub tick: u64,
    pub detail0: u64,
    pub detail1: u64,
    pub detail2: u64,
    pub detail3: u64,
    pub detail4: u64,
}

/// Additive `TaskSpawnImage` extended-attributes word: isolation class and
/// owner-environment id packed into the additive flag slot that already
/// carries the guest syscall-ABI magic. Legacy words (`spawn_abi::NATIVE`,
/// `spawn_abi::LINUX_SYSCALL`) never set bit 63, so old senders and old
/// kernels are unaffected; unknown/reserved bits are rejected loudly.
pub mod task_spawn_attrs {
    /// Bit marking a word as an extended spawn-attributes payload.
    pub const EXTENDED_FLAG: u64 = 1 << 63;
    /// Linux syscall-ABI bit inside an extended word (0 = native).
    pub const LINUX_ABI_FLAG: u64 = 1 << 0;
    /// Isolation class field: bits 8..16.
    pub const CLASS_SHIFT: u32 = 8;
    pub const CLASS_MASK: u64 = 0xff << CLASS_SHIFT;
    /// Isolation classes. `NONE` keeps the pre-isolation behavior exactly.
    pub const CLASS_NONE: u64 = 0;
    /// Guest-workload class: the kernel denies a defined dangerous-syscall
    /// set (spawn, raw block, raw NIC mutation) for tasks in this class.
    pub const CLASS_GUEST: u64 = 1;
    /// Owner-environment presence flag and id: bit 16, bits 24..40.
    pub const OWNER_ENV_FLAG: u64 = 1 << 16;
    pub const OWNER_ENV_SHIFT: u32 = 24;
    pub const OWNER_ENV_MASK: u64 = 0xffff << OWNER_ENV_SHIFT;
    /// Every bit the decoder knows about; anything else is rejected.
    pub const VALID_MASK: u64 =
        EXTENDED_FLAG | LINUX_ABI_FLAG | CLASS_MASK | OWNER_ENV_FLAG | OWNER_ENV_MASK;

    /// A decoded extended spawn-attributes payload.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct SpawnAttrs {
        pub linux_abi: bool,
        pub isolation_guest: bool,
        pub owner_env: Option<u16>,
    }

    /// Pack attributes into the additive flag word. Always sets the
    /// extended marker; the zero-attribute encoding is the guest-exec
    /// native default.
    pub const fn encode(attrs: SpawnAttrs) -> u64 {
        let mut word = EXTENDED_FLAG;
        if attrs.linux_abi {
            word |= LINUX_ABI_FLAG;
        }
        if attrs.isolation_guest {
            word |= CLASS_GUEST << CLASS_SHIFT;
        }
        if let Some(env_id) = attrs.owner_env {
            word |= OWNER_ENV_FLAG | ((env_id as u64) << OWNER_ENV_SHIFT);
        }
        word
    }

    /// Decode an additive flag word. `None` for legacy words (they must be
    /// decoded through the ABI-magic path) and for any word carrying bits
    /// outside `VALID_MASK`.
    pub const fn decode_extended(word: u64) -> Option<SpawnAttrs> {
        if word & EXTENDED_FLAG == 0 {
            return None;
        }
        if word & !VALID_MASK != 0 {
            return None;
        }
        let owner_env = if word & OWNER_ENV_FLAG != 0 {
            Some(((word & OWNER_ENV_MASK) >> OWNER_ENV_SHIFT) as u16)
        } else {
            None
        };
        Some(SpawnAttrs {
            linux_abi: word & LINUX_ABI_FLAG != 0,
            isolation_guest: (word & CLASS_MASK) >> CLASS_SHIFT == CLASS_GUEST,
            owner_env,
        })
    }

    /// True when `word` is a legacy ABI-magic flag word (never extended).
    pub const fn is_legacy(word: u64) -> bool {
        word & EXTENDED_FLAG == 0
    }
}

#[cfg(test)]
mod task_spawn_attrs_tests {
    use super::task_spawn_attrs::{self, CLASS_GUEST, EXTENDED_FLAG, OWNER_ENV_FLAG, SpawnAttrs};

    const LEGACY_NATIVE: u64 = 0;
    const LEGACY_LINUX: u64 = 0x534f_534c_494e_5558;

    #[test]
    fn legacy_words_are_never_extended() {
        assert!(task_spawn_attrs::is_legacy(LEGACY_NATIVE));
        assert!(task_spawn_attrs::is_legacy(LEGACY_LINUX));
        assert_eq!(task_spawn_attrs::decode_extended(LEGACY_NATIVE), None);
        assert_eq!(task_spawn_attrs::decode_extended(LEGACY_LINUX), None);
    }

    #[test]
    fn extended_roundtrip_carries_class_and_owner_env() {
        let attrs = SpawnAttrs {
            linux_abi: true,
            isolation_guest: true,
            owner_env: Some(3),
        };
        let word = task_spawn_attrs::encode(attrs);
        assert!(!task_spawn_attrs::is_legacy(word));
        assert_eq!(task_spawn_attrs::decode_extended(word), Some(attrs));
    }

    #[test]
    fn native_guest_without_owner_env_encodes_minimally() {
        let attrs = SpawnAttrs {
            linux_abi: false,
            isolation_guest: true,
            owner_env: None,
        };
        let word = task_spawn_attrs::encode(attrs);
        assert_eq!(word & EXTENDED_FLAG, EXTENDED_FLAG);
        assert_eq!(word & OWNER_ENV_FLAG, 0);
        assert_eq!(task_spawn_attrs::decode_extended(word), Some(attrs));
    }

    #[test]
    fn class_none_is_explicit_and_decodes() {
        let attrs = SpawnAttrs {
            linux_abi: false,
            isolation_guest: false,
            owner_env: Some(1),
        };
        let word = task_spawn_attrs::encode(attrs);
        assert_eq!(word & (CLASS_GUEST << 8), 0);
        assert_eq!(task_spawn_attrs::decode_extended(word), Some(attrs));
    }

    #[test]
    fn unknown_bits_are_rejected_loudly() {
        let poisoned = task_spawn_attrs::encode(SpawnAttrs {
            linux_abi: false,
            isolation_guest: true,
            owner_env: None,
        }) | (1 << 40);
        assert_eq!(task_spawn_attrs::decode_extended(poisoned), None);
    }

    #[test]
    fn owner_env_bounds_are_sixteen_bit() {
        let attrs = SpawnAttrs {
            linux_abi: false,
            isolation_guest: true,
            owner_env: Some(u16::MAX),
        };
        let word = task_spawn_attrs::encode(attrs);
        assert_eq!(task_spawn_attrs::decode_extended(word), Some(attrs));
    }
}

#[cfg(test)]
mod task_load_library_tests {
    use super::{SyscallNumber, TaskLoadedLibrary, task_load_library_flags};

    #[test]
    fn load_library_numbers_are_additive_appended() {
        // Row-164 runtime loads: appended after the previous highest
        // syscall number; older kernels see them as InvalidCall, older
        // binaries never emit them.
        assert_eq!(SyscallNumber::TaskLoadLibrary as u32, 56);
        assert_eq!(SyscallNumber::TaskSymbolLookup as u32, 57);
        assert!(SyscallNumber::TaskLoadLibrary as u32 > SyscallNumber::RngRequest as u32);
    }

    #[test]
    fn load_library_flags_accept_only_the_zero_word() {
        assert_eq!(task_load_library_flags::VALID_MASK, 0);
        assert_eq!(0u64 & !task_load_library_flags::VALID_MASK, 0);
        assert_ne!(1u64 & !task_load_library_flags::VALID_MASK, 0);
        assert_ne!(u64::MAX & !task_load_library_flags::VALID_MASK, 0);
    }

    #[test]
    fn loaded_library_record_roundtrips() {
        // Spawn companions: boot-store image id, handle 0.
        let companion = TaskLoadedLibrary {
            image_id: 7,
            library_handle: 0,
            base: 0x4000_0000_0100_0000,
            mapped_bytes: 0x3000,
        };
        let bytes: [u8; 24] = unsafe { core::mem::transmute(companion) };
        let decoded: TaskLoadedLibrary = unsafe { core::mem::transmute(bytes) };
        assert_eq!(decoded.image_id, 7);
        assert_eq!(decoded.library_handle, 0);
        assert_eq!(decoded.base, 0x4000_0000_0100_0000);
        assert_eq!(decoded.mapped_bytes, 0x3000);

        // Runtime loads: image id 0, TaskLoadLibrary handle.
        let runtime = TaskLoadedLibrary {
            image_id: 0,
            library_handle: 3,
            base: 0x6000_0000_0000,
            mapped_bytes: 0x2000,
        };
        let bytes: [u8; 24] = unsafe { core::mem::transmute(runtime) };
        let decoded: TaskLoadedLibrary = unsafe { core::mem::transmute(bytes) };
        assert_eq!(decoded.image_id, 0);
        assert_eq!(decoded.library_handle, 3);
    }
}
