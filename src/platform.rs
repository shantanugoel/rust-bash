//! Platform abstraction for types that differ between native and WASM.
//!
//! On native targets, re-exports from `std::time`.
//! On `wasm32`, re-exports from `web_time` which uses `js_sys::Date` under the hood.

#[cfg(target_arch = "wasm32")]
pub use web_time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Instant, SystemTime, UNIX_EPOCH};
