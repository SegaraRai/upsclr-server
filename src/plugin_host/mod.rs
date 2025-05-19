pub mod plugin_ffi;
pub mod plugin;
pub mod instance_manager;
pub mod plugin_host_impl;
mod entrypoint;

pub use entrypoint::main;
