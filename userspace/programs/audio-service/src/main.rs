#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;

mod consts;
mod protocol;
mod service;
mod types;
mod util;

rt::entry!(service::run);
