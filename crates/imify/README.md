# imify

Rust SDK for the imify platform API. The [`imify` CLI](https://github.com/imify-ai/imify-cli) is built on it.

```rust
use imify::Client;
use imify::auth::StaticToken;

let client = Client::builder("https://api.example.com")
    .token_source(StaticToken::new("<access token>"))
    .build()?;
```

`Client` sends `User-Agent: imify-cli/<version>` and `Authorization: Bearer <token>` with every request. It retries a failed request up to 3 times with exponential backoff: `GET`, `HEAD`, `PUT`, `DELETE` and completing an ingest session on network errors and server errors (5xx), every request on connection failures and rate limits (429). `RetryPolicy` changes the limits. It follows redirects with status 307 and 308 only. Every failure is an `imify::Error` variant, such as `Auth`, `NotFound`, `Validation` or `RateLimited`.

## Sign in

```rust
use std::time::Duration;
use imify::auth::{LoopbackLogin, StoredLogin};

let settings = client.oidc_settings().await?;
let login = LoopbackLogin::start(&client, &settings).await?;
eprintln!("Signing in at {}. Open {}", login.provider_host(), login.authorization_url());
let credentials = login.finish(Duration::from_secs(300)).await?;
store.save(&credentials)?;

let source = StoredLogin::load(store).await?.expect("credentials were saved");
let client = Client::builder("https://api.example.com").token_source(source).build()?;
```

`oidc_settings` reads the issuer and client id from the host's `config.json`. `LoopbackLogin` runs the authorization code flow with PKCE and receives the redirect on `http://localhost:<port>/callback`, on port 28753, 28754 or 28755. `PasteLogin` takes the pasted redirect URL instead. `Credentials` hold the ID token, which is the bearer token, and the refresh token. `store` is an `Arc<dyn CredentialStore>` you implement. `StoredLogin` refreshes the ID token when less than 60 s remain before it expires, and saves the result. `revoke` revokes the refresh token at logout.

## Upload a dataset

```rust
use imify::ingest::{NewSession, UploadEvent, UploadPlan, Uploader};

let plan = UploadPlan::from_dir("datasets/pick-place")?;
let session = Uploader::new(client)
    .on_event(|event| {
        if let UploadEvent::FileFinished { path, size } = event {
            println!("{path}: {size} bytes");
        }
    })
    .upload(&plan, &NewSession::new("pick-place"))
    .await?;
```

`upload` creates an ingest session, sends `meta/info.json`, sends the files under `meta/`, `data/` and `videos/` 4 at a time, and completes the session. Files are streamed from disk. Request starts are at least 100 ms apart; a rate-limited response holds all starts for the requested delay. `UploadOptions` changes the concurrency and the interval.

This crate is in early development. Its public API is not stable before 1.0.
