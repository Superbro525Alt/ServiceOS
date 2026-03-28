#![no_std]
#![no_main]

mod consts;
mod protocol;
mod service;
mod types;
mod util;

use serviceos_userspace_runtime as rt;

rt::entry!(main);

fn main() -> u64 {
    service::run()
}
