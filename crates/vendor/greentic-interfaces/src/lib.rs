#![deny(unsafe_code)]
#![warn(missing_docs, clippy::unwrap_used, clippy::expect_used)]
#![doc = include_str!("../README.md")]

//! ABI-oriented bindings for Greentic WIT packages.

/// Returns the canonical WIT root shipped with this crate.
#[must_use]
pub fn wit_root() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in [
        manifest_dir.join("../../wit"),
        manifest_dir.join("wit"),
        manifest_dir.join("../greentic-interfaces/wit"),
    ] {
        if has_wit_files(&candidate) {
            return candidate;
        }
    }
    manifest_dir.join("wit")
}

fn has_wit_files(root: &std::path::Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("wit") {
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub mod bindings;
#[cfg(not(target_arch = "wasm32"))]
pub mod wit_all;
#[cfg(all(
    feature = "bindings-rust",
    feature = "provider-common",
    not(target_arch = "wasm32")
))]
pub use bindings::provider_common_0_0_2_common;
#[cfg(not(target_arch = "wasm32"))]
#[allow(unused_imports)]
pub use wit_all::*;
#[cfg(all(feature = "bindings-rust", not(target_arch = "wasm32")))]
pub mod mappers;
#[cfg(all(feature = "bindings-rust", not(target_arch = "wasm32")))]
pub mod validate;
