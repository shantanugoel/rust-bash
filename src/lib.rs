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
pub use commands::{CommandContext, CommandResult, ExecCallback, VirtualCommand};
pub use error::{RustBashError, VfsError};
pub use interpreter::{
    ExecResult, ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, Variable,
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
