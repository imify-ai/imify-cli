//! Rust SDK for the imify platform API.
//!
//! The `imify` command-line tool is built on this crate; every request it makes goes
//! through the types defined here.
//!
//! ```
//! use imify::Client;
//! use imify::auth::StaticToken;
//!
//! let client = Client::builder("https://api.example.com")
//!     .token_source(StaticToken::new("secret"))
//!     .build()?;
//! # Ok::<(), imify::Error>(())
//! ```
#![deny(missing_docs)]

pub mod auth;
mod client;
mod error;
mod retry;

pub use client::{Client, ClientBuilder};
pub use error::{Error, FieldError};
pub use retry::RetryPolicy;

/// Version of this crate, as published.
///
/// ```
/// println!("imify SDK {}", imify::VERSION);
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
