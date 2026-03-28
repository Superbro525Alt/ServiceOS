#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;

mod config;
mod consts;
mod device;
mod protocol;
mod service;
mod types;
mod util;

rt::entry!(service::run);
