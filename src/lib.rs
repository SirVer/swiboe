#![feature(custom_derive, plugin)]
#![feature(drain)]
#![feature(result_expect)]
#![feature(scoped)]
#![plugin(serde_macros)]

// NOCOM(#sirver): move travis configuration into shell scripts like in UltiSnips.

extern crate libc;
extern crate mio;
extern crate serde;
extern crate uuid;

mod ipc_bridge;
pub mod client;
pub mod error;
pub mod ipc;
pub mod plugin_buffer;
pub mod plugin_core;
pub mod server;

pub use error::{Error, ErrorKind, Result};
