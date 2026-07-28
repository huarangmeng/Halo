//! Narrow Flutter-facing API for Halo.
//!
//! Flutter receives product-ready snapshots from this crate. Native platform
//! drivers submit opaque operating-system observations here and never parse the
//! Halo protocol themselves.

#![forbid(unsafe_code)]

mod api;

pub use api::*;
