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

`oidc_settings` reads the issuer and client id from the host's protected resource metadata (RFC 9728), and from its `config.json` when the host publishes no metadata. `LoopbackLogin` runs the authorization code flow with PKCE and receives the redirect on `http://localhost:<port>/callback`, on port 28753, 28754 or 28755. It accepts only the redirect that carries the login's `state` and answers others with 400. `PasteLogin` takes the pasted redirect URL instead. `Credentials` hold the ID token, which is the bearer token, the refresh token and the user's subject. `store` is an `Arc<dyn CredentialStore>` you implement. `StoredLogin` refreshes the ID token when less than 60 s remain before it expires, and saves the result; a refreshed ID token must name the same subject. `revoke` revokes the refresh token at logout.

The API URL, the issuer and every identity provider endpoint use `https`; `http` works only for `localhost`, `127.0.0.1` and `::1`. The protected resource metadata, `config.json` and the discovery document follow redirects only within their origin, token and revocation requests follow none, and their responses are limited to 1 MiB. Errors never contain a token, an authorization code or the PKCE verifier.

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

## Download a checkpoint

```rust
use imify::download::{DownloadSource, Downloader};

let summary = Downloader::new(client)
    .download(&DownloadSource::checkpoint("8d41c7", "step-20000"), "checkpoints/20000")
    .await?;
println!("{} files in {}", summary.files, summary.target.display());
```

`download` fetches the manifest of a training checkpoint and saves its files 4 at a time. Each file is written to `<path>.part`, checked against the manifest size, synced and renamed into place. Storage requests carry no `Authorization` header. The download refuses to start when a manifest path is unsafe, or when the target directory holds a symbolic link, a file not in the manifest or a manifest file with another size. Files that exist with the manifest size are skipped, so running the download again completes an interrupted one. A partial `<path>.part` is continued with a `Range` request when storage answers with 206 and the matching `Content-Range`. When storage rejects an expired URL with 403, the manifest is fetched again once.

## Update a program installed by its installer

```toml
imify = { version = "0.1", features = ["self-update"] }
```

```rust
use imify::update::{Installation, ReleaseClient, ReleaseSource, Version};

let source = ReleaseSource::imify_cli();
let receipt = source.receipt_path(&config_home); // $XDG_CONFIG_HOME or ~/.config
if let Some(installation) = Installation::from_receipt(&receipt, &std::env::current_exe()?, &source)? {
    let client = ReleaseClient::builder(source).build()?;
    let latest = client.latest().await?;
    if latest.version > Version::parse(env!("CARGO_PKG_VERSION"))? {
        client.install(&installation, &latest).await?;
    }
}
```

The `self-update` feature adds `imify::update`. `Installation::from_receipt` reads the install receipt of the dist shell installer and returns the install when it contains the executable. `latest` sends `GET /repos/<owner>/<repo>/releases/latest` to the GitHub API. `install` downloads `<app>-installer.sh` of that release from `https://github.com` and runs it with `/bin/sh` for the receipt's install prefix. The module reads no environment variables; the caller passes the configuration directory and, for GitHub Enterprise Server, the API and download URLs.

This crate is in early development. Its public API is not stable before 1.0.
