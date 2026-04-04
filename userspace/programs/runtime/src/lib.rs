#![no_std]

pub use serviceos_abi::*;

mod app_control;
mod audio;
mod bootstrap;
mod clipboard;
mod config;
mod developer;
mod compat;
mod console;
mod desktop;
mod devices;
mod decode;
mod graphics;
mod glyphs;
mod ipc;
mod kernel;
mod log_service;
mod manager;
mod memory;
mod network;
mod package;
mod relay;
mod runtime_core;
mod session;
mod security;
mod status;
mod storage;
mod terminal;
mod types;

pub use app_control::*;
pub use audio::*;
pub use bootstrap::*;
pub use clipboard::*;
pub use config::*;
pub use developer::*;
pub use compat::*;
pub use console::*;
pub use desktop::*;
pub use devices::*;
pub use graphics::*;
pub use glyphs::*;
pub use ipc::*;
pub use kernel::*;
pub use log_service::*;
pub use manager::*;
pub use memory::*;
pub use network::*;
pub use package::*;
pub use relay::*;
pub use runtime_core::*;
pub use session::*;
pub use security::*;
pub use status::*;
pub use storage::*;
pub use terminal::*;
pub use types::*;
pub use serviceos_abi::{app_permission, audio_capability, input_capability, rights, runtime_capability};

pub(crate) use decode::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unsupported,
    InvalidCall,
    PermissionDenied,
    NotInitialized,
    InvalidArgument,
    BufferTooSmall,
    QueueEmpty,
    NotFound,
    Busy,
    CapacityExceeded,
    Unknown(u64),
}

impl Error {
    fn from_code(code: u64) -> Self {
        match code {
            x if x == SyscallErrorCode::Unsupported as u64 => Self::Unsupported,
            x if x == SyscallErrorCode::InvalidCall as u64 => Self::InvalidCall,
            x if x == SyscallErrorCode::PermissionDenied as u64 => Self::PermissionDenied,
            x if x == SyscallErrorCode::NotInitialized as u64 => Self::NotInitialized,
            x if x == SyscallErrorCode::InvalidArgument as u64 => Self::InvalidArgument,
            x if x == SyscallErrorCode::BufferTooSmall as u64 => Self::BufferTooSmall,
            x if x == SyscallErrorCode::QueueEmpty as u64 => Self::QueueEmpty,
            x if x == SyscallErrorCode::NotFound as u64 => Self::NotFound,
            x if x == SyscallErrorCode::Busy as u64 => Self::Busy,
            x if x == SyscallErrorCode::CapacityExceeded as u64 => Self::CapacityExceeded,
            other => Self::Unknown(other),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

#[macro_export]
macro_rules! entry {
    ($path:path) => {
        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            let _ = $crate::write_log("panic", "userspace panic");
            $crate::thread_exit(0xffff_ffff_ffff_ff00)
        }

        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text.start")]
        pub extern "C" fn _start() -> ! {
            let code: u64 = $path();
            $crate::thread_exit(code)
        }
    };
}
