#![forbid(unsafe_code)]

pub mod archive_shape;
pub mod builder;
pub mod builtin;
pub mod events;
pub mod kind;
pub mod messaging;
pub mod pack_lock;
#[cfg(feature = "native")]
pub mod path_safety;
pub mod plan;
#[cfg(feature = "native")]
pub mod reader;
pub mod repo;
#[cfg(feature = "native")]
pub mod resolver;
pub mod static_routes;
#[cfg(feature = "native")]
pub mod validate;

pub use archive_shape::{
    CANONICAL_MANIFEST_ENTRY, DW_MANIFEST_ENTRY, DW_METADATA_ENTRY, PackArchiveShape,
    archive_shape_is_ambiguous, detect_archive_shape,
};
pub use kind::PackKind;
pub use pack_lock::*;
#[cfg(feature = "native")]
pub use reader::*;
#[cfg(feature = "native")]
pub use resolver::*;
