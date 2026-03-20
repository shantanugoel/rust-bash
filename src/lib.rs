pub mod api;
pub mod commands;
pub mod error;
pub mod interpreter;
pub mod vfs;

pub use api::{RustBash, RustBashBuilder};
pub use commands::{CommandContext, CommandResult, ExecCallback, VirtualCommand};
pub use error::{RustBashError, VfsError};
pub use interpreter::{
    ExecResult, ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, Variable,
};
pub use vfs::{InMemoryFs, VirtualFs};

#[cfg(test)]
mod parser_smoke_tests;
