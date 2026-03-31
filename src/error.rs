use std::fmt;
use std::path::PathBuf;

/// Errors arising from virtual filesystem operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsError {
    NotFound(PathBuf),
    AlreadyExists(PathBuf),
    NotADirectory(PathBuf),
    NotAFile(PathBuf),
    IsADirectory(PathBuf),
    PermissionDenied(PathBuf),
    DirectoryNotEmpty(PathBuf),
    SymlinkLoop(PathBuf),
    InvalidPath(String),
    IoError(String),
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VfsError::NotFound(p) => write!(f, "No such file or directory: {}", p.display()),
            VfsError::AlreadyExists(p) => write!(f, "Already exists: {}", p.display()),
            VfsError::NotADirectory(p) => write!(f, "Not a directory: {}", p.display()),
            VfsError::NotAFile(p) => write!(f, "Not a file: {}", p.display()),
            VfsError::IsADirectory(p) => write!(f, "Is a directory: {}", p.display()),
            VfsError::PermissionDenied(p) => write!(f, "Permission denied: {}", p.display()),
            VfsError::DirectoryNotEmpty(p) => write!(f, "Directory not empty: {}", p.display()),
            VfsError::SymlinkLoop(p) => {
                write!(f, "Too many levels of symbolic links: {}", p.display())
            }
            VfsError::InvalidPath(msg) => write!(f, "Invalid path: {msg}"),
            VfsError::IoError(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for VfsError {}

/// Top-level error type for the rust-bash interpreter.
#[derive(Debug)]
pub enum RustBashError {
    Parse(String),
    Execution(String),
    /// An expansion-time error that aborts the current command and sets the
    /// exit code.  When `should_exit` is true the *script* also terminates
    /// (used by `${var:?msg}`).  When false, only the current command is
    /// aborted (used for e.g. negative substring length).
    ExpansionError {
        message: String,
        exit_code: i32,
        should_exit: bool,
    },
    /// A failglob error: no glob matches found when `shopt -s failglob` is on.
    /// Aborts the current simple command (exit code 1) but does NOT exit the script.
    FailGlob {
        pattern: String,
    },
    LimitExceeded {
        limit_name: &'static str,
        limit_value: usize,
        actual_value: usize,
    },
    Network(String),
    Vfs(VfsError),
    Timeout,
}

impl fmt::Display for RustBashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RustBashError::Parse(msg) => write!(f, "parse error: {msg}"),
            RustBashError::Execution(msg) => write!(f, "execution error: {msg}"),
            RustBashError::ExpansionError { message, .. } => {
                write!(f, "expansion error: {message}")
            }
            RustBashError::FailGlob { pattern } => {
                write!(f, "no match: {pattern}")
            }
            RustBashError::LimitExceeded {
                limit_name,
                limit_value,
                actual_value,
            } => write!(
                f,
                "limit exceeded: {limit_name} ({actual_value}) exceeded limit ({limit_value})"
            ),
            RustBashError::Network(msg) => write!(f, "network error: {msg}"),
            RustBashError::Vfs(e) => write!(f, "vfs error: {e}"),
            RustBashError::Timeout => write!(f, "execution timed out"),
        }
    }
}

impl std::error::Error for RustBashError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RustBashError::Vfs(e) => Some(e),
            _ => None,
        }
    }
}

impl From<VfsError> for RustBashError {
    fn from(e: VfsError) -> Self {
        RustBashError::Vfs(e)
    }
}
