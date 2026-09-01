#![no_std]

//! Legacy PC interrupt-controller bring-up shared by the x86 platform images.
//!
//! Both x86 targets boot in the same PIC/PIT-era virtual-wire mode, so the
//! 8259 remap, PIT programming, and PIT-derived reference intervals live
//! here, behind the [`ExternalInterruptOps`] seam the arch crate consumes.

mod legacy;

pub use legacy::external_ops;
