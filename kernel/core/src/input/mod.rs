mod backend;
mod core;

pub use backend::{InputBackend, InputSourceError, InputSourceObject};
pub use core::{InputCore, initialize, manager};
