//! Shared build/run core for ServiceOS development tooling.
//!
//! Hosted out of the `xtask` binary crate so the e2e runner framework can
//! reuse the canonical QEMU command builders, boot-log driver, platform
//! specs, and image staging without duplicating argv assembly. The modules
//! here are byte-identical to their historical locations; `cargo xtask`
//! behavior is unchanged (docs/test-plan.md §2.3).

pub mod bootlog;
pub mod build;
pub mod bundle;
pub mod image;
pub mod platform;
pub mod run;
