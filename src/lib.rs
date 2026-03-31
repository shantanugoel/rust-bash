//! A sandboxed bash interpreter with a virtual filesystem.
//!
//! `rust-bash` executes bash scripts safely in-process — no containers, no VMs,
//! no host access. All file operations happen on a pluggable virtual filesystem
//! (in-memory by default), and configurable execution limits prevent runaway scripts.
//!
//! # Quick start
//!
//! ```rust
//! use rust_bash::RustBashBuilder;
//! use std::collections::HashMap;
//!
//! let mut shell = RustBashBuilder::new()
//!     .files(HashMap::from([
//!         ("/hello.txt".into(), b"hello world".to_vec()),
//!     ]))
//!     .build()
//!     .unwrap();
//!
//! let result = shell.exec("cat /hello.txt").unwrap();
//! assert_eq!(result.stdout, "hello world");
//! assert_eq!(result.exit_code, 0);
//! ```
//!
//! # Features
//!
//! - **80+ built-in commands** — echo, cat, grep, awk, sed, jq, find, sort, diff, curl, and more
//! - **Full bash syntax** — pipelines, redirections, variables, control flow, functions,
//!   command substitution, globs, brace expansion, arithmetic, here-documents, case statements
//! - **Execution limits** — 10 configurable bounds (time, commands, loops, output size, etc.)
//! - **Network policy** — sandboxed `curl` with URL allow-lists and method restrictions
//! - **Multiple filesystem backends** — [`InMemoryFs`], [`OverlayFs`], [`ReadWriteFs`], [`MountableFs`]
//! - **Custom commands** — implement the [`VirtualCommand`] trait to add your own
//! - **C FFI and WASM** — embed in any language via shared library or WebAssembly

pub mod api;
pub mod commands;
pub mod error;
pub mod interpreter;
pub mod platform;
pub mod vfs;

#[cfg(feature = "network")]
pub mod network;
#[cfg(not(feature = "network"))]
pub mod network {
    //! Stub network module when the `network` feature is disabled.
    //! NOTE: This struct must stay in sync with `src/network.rs`.
    use std::collections::HashSet;
    use std::time::Duration;

    #[derive(Clone, Debug)]
    pub struct NetworkPolicy {
        pub enabled: bool,
        pub allowed_url_prefixes: Vec<String>,
        pub allowed_methods: HashSet<String>,
        pub max_redirects: usize,
        pub max_response_size: usize,
        pub timeout: Duration,
    }

    impl Default for NetworkPolicy {
        fn default() -> Self {
            Self {
                enabled: false,
                allowed_url_prefixes: Vec::new(),
                allowed_methods: HashSet::from(["GET".to_string(), "POST".to_string()]),
                max_redirects: 5,
                max_response_size: 10 * 1024 * 1024,
                timeout: Duration::from_secs(30),
            }
        }
    }

    impl NetworkPolicy {
        pub fn validate_url(&self, _url: &str) -> Result<(), String> {
            Err("network feature is disabled".to_string())
        }

        pub fn validate_method(&self, _method: &str) -> Result<(), String> {
            Err("network feature is disabled".to_string())
        }
    }
}

pub use api::{RustBash, RustBashBuilder};
pub use commands::{CommandContext, CommandMeta, CommandResult, ExecCallback, VirtualCommand};
pub use error::{RustBashError, VfsError};
pub use interpreter::{
    ExecResult, ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, Variable,
    VariableAttrs, VariableValue, builtin_names,
};
pub use network::NetworkPolicy;
pub use vfs::{DirEntry, InMemoryFs, Metadata, MountableFs, NodeType, VirtualFs};

#[cfg(feature = "native-fs")]
pub use vfs::{OverlayFs, ReadWriteFs};

#[cfg(feature = "ffi")]
pub mod ffi;

#[cfg(feature = "cli")]
pub mod mcp;

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(test)]
mod parser_smoke_tests;
