//! Rust SDK for the imify platform API.
//!
//! The `imify` command-line tool is built on this crate; every request it makes goes
//! through the types defined here.
//!
//! ```
//! assert!(!imify::VERSION.is_empty());
//! ```
#![deny(missing_docs)]

/// Version of this crate, as published.
///
/// ```
/// println!("imify SDK {}", imify::VERSION);
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
