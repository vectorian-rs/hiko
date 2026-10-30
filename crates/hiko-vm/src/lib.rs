pub mod budget;
pub mod builder;
pub mod builtins;
pub mod config;
pub mod heap;
pub mod io_backend;
pub mod io_limits;
#[cfg(test)]
mod lifecycle_tests;
pub mod process;
pub mod runtime;
pub mod runtime_ops;
pub mod scheduler;
pub mod sendable;
pub mod shape_check;
mod sync;
#[cfg(test)]
mod test_support;
pub mod threaded;
pub mod value;
pub mod verify;
pub mod vm;

pub use vm::{DEFAULT_MAX_CALL_FRAMES, DEFAULT_MAX_STACK_SLOTS};
