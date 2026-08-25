#![cfg_attr(not(test), no_std)]

//! Pure installer/updater model shared between the service binary and host
//! unit tests. See [`ops_model`] for details.

pub mod ops_model;
pub mod signing;

pub use ops_model::*;
