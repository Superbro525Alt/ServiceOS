#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod consts;
mod protocol;
mod registry;
mod routing;
mod sandbox;
mod service;
mod types;
mod util;

use serviceos_userspace_runtime as rt;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    service::run()
}
