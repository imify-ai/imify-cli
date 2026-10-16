# imify

Rust SDK for the imify platform API. The [`imify` CLI](https://github.com/imify-ai/imify-cli) is built on it.

```rust
use imify::Client;
use imify::auth::StaticToken;

let client = Client::builder("https://api.example.com")
    .token_source(StaticToken::new("<access token>"))
    .build()?;
```

`Client` sends `User-Agent: imify-cli/<version>` and `Authorization: Bearer <token>` with every request. It retries a failed request up to 3 times with exponential backoff: `GET`, `HEAD`, `PUT` and `DELETE` on network errors and server errors (5xx), every method on connection failures and rate limits (429). `RetryPolicy` changes the limits. Every failure is an `imify::Error` variant, such as `Auth`, `NotFound`, `Validation` or `RateLimited`.

This crate is in early development. Its public API is not stable before 1.0.
