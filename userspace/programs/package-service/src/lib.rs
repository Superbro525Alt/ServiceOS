#![cfg_attr(not(test), no_std)]

//! Pure installer/updater model shared between the service binary and host
//! unit tests. See [`ops_model`] for details.

pub mod ops_model;
pub mod rollout;
pub mod signing;
pub mod sysupdate_model;

pub use ops_model::*;
pub use sysupdate_model::*;
