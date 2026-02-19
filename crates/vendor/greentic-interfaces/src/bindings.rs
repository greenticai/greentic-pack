#![allow(clippy::all)]
#![allow(missing_docs)]
#![allow(unsafe_code)]
#![allow(unused_imports)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

/// Rust bindings generated from the Greentic WIT worlds.
/// The build script emits `GREENTIC_INTERFACES_BINDINGS` pointing inside `OUT_DIR`,
/// so the include is always target/profile-specific and cache-safe.
#[cfg(feature = "bindings-rust")]
pub mod generated {
    include!(concat!(env!("GREENTIC_INTERFACES_BINDINGS"), "/mod.rs"));
}

#[cfg(feature = "bindings-rust")]
pub use generated::*;
