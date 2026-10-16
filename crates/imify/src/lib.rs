//! Rust SDK for the imify platform API.
//!
//! The `imify` command-line tool is built on this crate; every request it makes goes
//! through the types defined here.
//!
//! ```no_run
//! use imify::Client;
//! use imify::auth::StaticToken;
//!
//! # async fn run() -> Result<(), imify::Error> {
//! let client = Client::builder("https://api.example.com")
//!     .token_source(StaticToken::new("secret"))
//!     .build()?;
//! for dataset in client.datasets().list().await? {
//!     println!("{} {}", dataset.id, dataset.task);
//! }
//! # Ok(()) }
//! ```
#![deny(missing_docs)]

mod account;
pub mod auth;
mod client;
pub mod datasets;
mod discovery;
mod error;
mod health;
pub mod ingest;
mod retry;
pub mod validate;

pub use account::User;
pub use client::{Client, ClientBuilder};
pub use discovery::OidcSettings;
pub use error::{Error, FieldError, JournalError, LoginFailure};
pub use health::{Health, Readiness};
pub use retry::RetryPolicy;

/// Version of this crate, as published.
///
/// ```
/// println!("imify SDK {}", imify::VERSION);
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
