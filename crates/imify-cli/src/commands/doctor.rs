//! `imify doctor`: checks the CLI configuration, the API, the access token and the identity
//! provider settings.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use imify::auth::{CredentialStore, StoredLogin, TokenSource};
use imify::{Client, ClientBuilder, OidcSettings, Readiness, RetryPolicy, User};
use url::Url;

use super::auth::{AuthSource, LoginLookup, TokenOrigin, oidc_settings, on_blocking_thread};
use super::write_result;
use crate::config::{ResolvedApiUrl, Source, is_loopback, resolve_api_url};
use crate::credentials::{HostStore, Keychain, Location, StoreChoice, host_key};
use crate::dirs::config_dir;
use crate::output::renderer::{Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::time::rfc3339;
use crate::output::{
    AUTH_REJECTED_HINT, AUTH_REQUIRED_HINT, CliError, Environment, ErrorCode, LOGIN_HINT,
};

/// Connect and read timeout of every request doctor sends.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Largest difference between the local clock and the server's `Date` that passes.
const CLOCK_SKEW_LIMIT_MS: i64 = 60_000;
/// A token that expires within this time gets a warning.
const EXPIRY_WARNING: Duration = Duration::from_secs(5 * 60);

/// `imify doctor`: runs every check and prints the report.
///
/// # Errors
///
/// After printing the report, returns the error code and hint of the first failing check,
/// with a message that names every failing check. Returns `internal` when stdout cannot be
/// written.
pub async fn doctor(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let report = diagnose(env).await;
    write_result(renderer, &report)?;
    outcome(&report.checks)
}

/// The result of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// One line of the report. `code` is set for `fail` only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Check {
    name: &'static str,
    status: Status,
    message: String,
    code: Option<ErrorCode>,
    hint: Option<String>,
}

impl Check {
    fn ok(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            message: message.into(),
            code: None,
            hint: None,
        }
    }

    fn warn(name: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            message: message.into(),
            code: None,
            hint: Some(hint.into()),
        }
    }

    fn fail(name: &'static str, error: CliError) -> Self {
        Self {
            name,
            status: Status::Fail,
            message: error.message,
            code: Some(error.code),
            hint: error.hint,
        }
    }

    fn skip(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Skip,
            message: message.into(),
            code: None,
            hint: None,
        }
    }

    /// A `skip` because `dependency` did not pass.
    fn skip_after(name: &'static str, dependency: &str) -> Self {
        Self::skip(name, format!("Skipped because {dependency} did not pass."))
    }

    /// A `warn` with the message and hint of `error`.
    fn warn_of(name: &'static str, error: CliError) -> Self {
        Self {
            name,
            status: Status::Warn,
            message: error.message,
            code: None,
            hint: error.hint,
        }
    }
}

/// The `clock` check of `skew_ms`, the server time minus the local time.
fn clock_check(skew_ms: i64) -> Check {
    if skew_ms.abs() <= CLOCK_SKEW_LIMIT_MS {
        return Check::ok("clock", "The local clock is within 60 s of the server.");
    }
    let direction = if skew_ms > 0 { "behind" } else { "ahead of" };
    Check::warn(
        "clock",
        format!(
            "The local clock is {} s {direction} the server.",
            skew_ms.unsigned_abs() / 1000
        ),
        "Enable time synchronization in the system settings.",
    )
}

/// The `token_expiry` check of a token that expires at `expires_at`, evaluated at `now`.
fn expiry_check(expires_at: SystemTime, now: SystemTime) -> Check {
    let timestamp = rfc3339(expires_at);
    match expires_at.duration_since(now) {
        Ok(remaining) if remaining > EXPIRY_WARNING => {
            Check::ok("token_expiry", format!("Expires at {timestamp}."))
        }
        Ok(remaining) if !remaining.is_zero() => Check::warn(
            "token_expiry",
            format!("Expires at {timestamp}, in less than 5 min."),
            "Set `IMIFY_TOKEN` to a new token before it expires.",
        ),
        _ => Check::fail(
            "token_expiry",
            CliError::new(
                ErrorCode::AuthRejected,
                format!("The token expired at {timestamp}."),
            )
            .with_hint(AUTH_REJECTED_HINT),
        ),
    }
}

/// The `token_expiry` check of a stored login whose ID token expires at `expires_at`,
/// evaluated at `now`. With a refresh token, an ID token that expires soon or has expired
/// passes, because the refresh token renews it.
fn login_expiry_check(expires_at: SystemTime, has_refresh_token: bool, now: SystemTime) -> Check {
    let timestamp = rfc3339(expires_at);
    let remaining = expires_at
        .duration_since(now)
        .ok()
        .filter(|remaining| !remaining.is_zero());
    match (remaining, has_refresh_token) {
        (Some(remaining), _) if remaining > EXPIRY_WARNING => {
            Check::ok("token_expiry", format!("Expires at {timestamp}."))
        }
        (Some(_), true) => Check::ok(
            "token_expiry",
            format!("Expires at {timestamp}, in less than 5 min. The refresh token renews it."),
        ),
        (Some(_), false) => Check::warn(
            "token_expiry",
            format!(
                "Expires at {timestamp}, in less than 5 min, and the login has no refresh token."
            ),
            LOGIN_HINT,
        ),
        (None, true) => Check::ok(
            "token_expiry",
            format!("Expired at {timestamp}. The refresh token renews it."),
        ),
        (None, false) => Check::fail(
            "token_expiry",
            CliError::new(
                ErrorCode::AuthExpired,
                format!("The login expired at {timestamp} and has no refresh token."),
            )
            .with_hint(LOGIN_HINT),
        ),
    }
}

/// Returns `true` when `issuer` is an `https` URL.
fn issuer_https(issuer: &str) -> bool {
    Url::parse(issuer).is_ok_and(|url| url.scheme() == "https")
}

/// The `oidc` check of the identity provider settings: `warn` when the issuer is neither
/// `https` nor `http` on `localhost`, `127.0.0.1` or `::1`.
fn oidc_check(settings: &OidcSettings) -> Check {
    let message = format!("Issuer {}, client {}.", settings.issuer, settings.client_id);
    let secure = Url::parse(&settings.issuer)
        .is_ok_and(|url| url.scheme() == "https" || (url.scheme() == "http" && is_loopback(&url)));
    if secure {
        Check::ok("oidc", message)
    } else {
        Check::warn(
            "oidc",
            format!("{message} The issuer does not use https."),
            "Set `IMIFY_OIDC_ISSUER` to an `https` issuer.",
        )
    }
}

/// `Ok` when no check failed. Otherwise the error code and hint of the first failing check,
/// with a message that names every failing check.
fn outcome(checks: &[Check]) -> Result<(), CliError> {
    let failed: Vec<&Check> = checks
        .iter()
        .filter(|check| check.status == Status::Fail)
        .collect();
    let Some(first) = failed.first() else {
        return Ok(());
    };
    let names: Vec<&str> = failed.iter().map(|check| check.name).collect();
    let noun = if failed.len() == 1 { "check" } else { "checks" };
    let error = CliError::new(
        first.code.unwrap_or(ErrorCode::Internal),
        format!("{} {noun} failed: {}.", failed.len(), names.join(", ")),
    );
    Err(match &first.hint {
        Some(hint) => error.with_hint(hint),
        None => error,
    })
}

/// `later` minus `earlier` in milliseconds, negative when `later` is earlier.
fn signed_millis(later: SystemTime, earlier: SystemTime) -> i64 {
    let millis = |duration: Duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    match later.duration_since(earlier) {
        Ok(ahead) => millis(ahead),
        Err(behind) => -millis(behind.duration()),
    }
}

/// A client for `url` that does not retry and gives up after [`TIMEOUT`] without progress.
fn client_builder(url: &str) -> ClientBuilder {
    Client::builder(url)
        .retry_policy(RetryPolicy::none())
        .connect_timeout(TIMEOUT)
        .read_timeout(TIMEOUT)
}

/// Everything doctor found out: the checks in report order and the values they produced.
#[derive(Debug, Default)]
struct Report {
    api_url: Option<String>,
    api_url_source: Option<Source>,
    latency_ms: Option<u64>,
    clock_skew_ms: Option<i64>,
    readiness: Option<Readiness>,
    token_source: Option<&'static str>,
    token_expires_at: Option<SystemTime>,
    user: Option<User>,
    credential_store: Option<StoreChoice>,
    config_dir: Option<PathBuf>,
    login: Option<LoginReport>,
    oidc: Option<OidcSettings>,
    checks: Vec<Check>,
}

/// The stored login the `token` check found.
#[derive(Debug)]
struct LoginReport {
    location: Location,
    has_refresh_token: bool,
}

impl Report {
    fn push(&mut self, check: Check) {
        self.checks.push(check);
    }
}

/// What a passing `health` check observed.
#[derive(Debug)]
struct Reachable {
    /// The server's `Date` header.
    server_time: Option<SystemTime>,
    /// The local time at which the response arrived.
    received_at: SystemTime,
}

async fn diagnose(env: &Environment) -> Report {
    let mut report = Report {
        credential_store: StoreChoice::parse(env.credential_store.as_deref()).ok(),
        config_dir: config_dir(env).ok(),
        ..Report::default()
    };

    let api = check_api_url(env, &mut report);
    let reachable = if let Some((_, client)) = &api {
        check_health(client, &mut report).await
    } else {
        report.push(Check::skip_after("health", "api_url"));
        None
    };
    match (&api, &reachable) {
        (Some((_, client)), Some(_)) => check_readiness(client, &mut report).await,
        _ => report.push(Check::skip_after("readiness", "health")),
    }
    check_clock(reachable.as_ref(), &mut report);

    let host = api.as_ref().map(|(api_url, _)| host_key(&api_url.url));
    let token_source = check_token(env, host.as_deref(), &mut report).await;
    match (&api, &host, token_source) {
        (_, _, None) => report.push(Check::skip_after("account", "token")),
        (Some((api_url, _)), Some(host), Some((source, origin))) if reachable.is_some() => {
            check_account(env, api_url, host, source, origin, &mut report).await;
        }
        _ => report.push(Check::skip_after("account", "health")),
    }

    match (&api, &host) {
        (Some((_, client)), Some(host)) if reachable.is_some() || oidc_overrides_set(env) => {
            check_oidc(env, client, host, &mut report).await;
        }
        (Some(_), _) => report.push(Check::skip_after("oidc", "health")),
        (None, _) => report.push(Check::skip_after("oidc", "api_url")),
    }
    report
}

/// Returns `true` when `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID` are both set and not
/// blank, so the `oidc` check sends no request.
fn oidc_overrides_set(env: &Environment) -> bool {
    [&env.oidc_issuer, &env.oidc_client_id]
        .into_iter()
        .all(|value| {
            value
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        })
}

/// Runs the `oidc` check on the settings `imify auth login` would use. Every problem is a
/// warning: it affects `imify auth login` only, not the token in use.
async fn check_oidc(env: &Environment, client: &Client, host: &str, report: &mut Report) {
    let check = match oidc_settings(env, client, host).await {
        Ok(settings) => {
            let check = oidc_check(&settings);
            report.oidc = Some(settings);
            check
        }
        Err(error) => Check::warn_of("oidc", error),
    };
    report.push(check);
}

/// Runs the `api_url` check and returns the URL with a client without a token source.
fn check_api_url(env: &Environment, report: &mut Report) -> Option<(ResolvedApiUrl, Client)> {
    let checked = resolve_api_url(env).and_then(|api_url| {
        client_builder(&api_url.to_string())
            .build()
            .map(|client| (api_url.clone(), client))
            .map_err(|error| api_url.invalid(&error))
    });
    match checked {
        Ok((api_url, client)) => {
            let url = api_url.to_string();
            report.push(Check::ok(
                "api_url",
                format!("{url} ({})", api_url.source.api_url_label()),
            ));
            report.api_url = Some(url);
            report.api_url_source = Some(api_url.source);
            Some((api_url, client))
        }
        Err(error) => {
            report.push(Check::fail("api_url", error));
            None
        }
    }
}

/// Runs the `health` check and returns what it observed when it passes.
async fn check_health(client: &Client, report: &mut Report) -> Option<Reachable> {
    let started = Instant::now();
    let result = client.health().await;
    let latency = started.elapsed();
    let received_at = SystemTime::now();
    let health = match result {
        Ok(health) => health,
        Err(error) => {
            report.push(Check::fail("health", error.into()));
            return None;
        }
    };
    report.latency_ms = Some(u64::try_from(latency.as_millis()).unwrap_or(u64::MAX));
    if health.status != "ok" {
        report.push(Check::fail(
            "health",
            CliError::new(
                ErrorCode::Server,
                format!("The API reports the status {}.", health.status),
            ),
        ));
        return None;
    }
    report.push(Check::ok(
        "health",
        format!("Reachable in {} ms.", latency.as_millis()),
    ));
    Some(Reachable {
        server_time: health.server_time,
        received_at,
    })
}

async fn check_readiness(client: &Client, report: &mut Report) {
    let check = match client.readiness().await {
        Ok(readiness) => {
            let check = readiness_check(&readiness);
            report.readiness = Some(readiness);
            check
        }
        Err(error) => Check::fail("readiness", error.into()),
    };
    report.push(check);
}

/// The `readiness` check of a readiness document.
fn readiness_check(readiness: &Readiness) -> Check {
    let (db, storage) = (&readiness.db, &readiness.storage);
    if readiness.status == "ok" {
        Check::ok("readiness", format!("Database {db}, storage {storage}."))
    } else {
        Check::fail(
            "readiness",
            CliError::new(
                ErrorCode::Server,
                format!("The API is not ready: database {db}, storage {storage}."),
            ),
        )
    }
}

/// Runs the `clock` check on what the `health` check observed.
fn check_clock(reachable: Option<&Reachable>, report: &mut Report) {
    let check = match reachable {
        Some(Reachable {
            server_time: Some(server_time),
            received_at,
        }) => {
            let skew_ms = signed_millis(*server_time, *received_at);
            report.clock_skew_ms = Some(skew_ms);
            clock_check(skew_ms)
        }
        Some(_) => Check::skip("clock", "Skipped because the response has no Date header."),
        None => Check::skip_after("clock", "health"),
    };
    report.push(check);
}

/// Runs the `token` and `token_expiry` checks and returns the token source with its origin
/// when it supplies a token. `host` is the host key of the API URL; without it, only
/// `IMIFY_TOKEN` is checked. A stored login is read from the store and never refreshed here.
async fn check_token(
    env: &Environment,
    host: Option<&str>,
    report: &mut Report,
) -> Option<(AuthSource, TokenOrigin)> {
    let found = match super::auth::env_token_source() {
        Ok(Some((source, _))) => check_env_token(env, host, source, report).await,
        Ok(None) => match host {
            Some(host) => check_stored_login(env, host, report).await,
            None => Err(CliError::new(
                ErrorCode::AuthRequired,
                format!("{} is not set.", imify::auth::EnvToken::VARIABLE),
            )
            .with_hint(AUTH_REQUIRED_HINT)),
        },
        Err(error) => Err(error),
    };
    match found {
        Ok(found) => Some(found),
        Err(error) => {
            report.push(Check::fail("token", error));
            report.push(Check::skip_after("token_expiry", "token"));
            None
        }
    }
}

/// The `token` and `token_expiry` checks of the token in `IMIFY_TOKEN`.
async fn check_env_token(
    env: &Environment,
    host: Option<&str>,
    source: AuthSource,
    report: &mut Report,
) -> Result<(AuthSource, TokenOrigin), CliError> {
    let token = source.token().await?;
    report.push(Check::ok(
        "token",
        format!("{} is set.", imify::auth::EnvToken::VARIABLE),
    ));
    report.token_source = Some("env");
    let expiry = match token.expires_at() {
        Some(expires_at) => {
            report.token_expires_at = Some(expires_at);
            expiry_check(expires_at, SystemTime::now())
        }
        None => Check::skip(
            "token_expiry",
            "Skipped because the token has no readable expiry.",
        ),
    };
    let expiry = match host {
        Some(host) => with_origin_hint(expiry, TokenOrigin::Env, env, host).await,
        None => expiry,
    };
    report.push(expiry);
    Ok((source, TokenOrigin::Env))
}

/// The `token` and `token_expiry` checks of the stored login of `host`.
async fn check_stored_login(
    env: &Environment,
    host: &str,
    report: &mut Report,
) -> Result<(AuthSource, TokenOrigin), CliError> {
    let store = Arc::new(HostStore::open(env, host, Keychain::system())?);
    let loading = Arc::clone(&store);
    let found = on_blocking_thread(move || loading.load_reporting()).await?;
    let not_logged_in = || not_logged_in(host, &store.locations());
    let (credentials, location) = found.ok_or_else(not_logged_in)?;
    let source = StoredLogin::load(Arc::clone(&store) as Arc<dyn CredentialStore>)
        .await?
        .ok_or_else(not_logged_in)?;
    let has_refresh_token = credentials.has_refresh_token();
    report.push(Check::ok(
        "token",
        stored_login_message(host, &location, has_refresh_token),
    ));
    report.token_source = Some("stored");
    let expires_at = credentials.expires_at();
    report.token_expires_at = Some(expires_at);
    report.push(login_expiry_check(
        expires_at,
        has_refresh_token,
        SystemTime::now(),
    ));
    report.login = Some(LoginReport {
        location,
        has_refresh_token,
    });
    Ok((AuthSource::Stored(Box::new(source)), TokenOrigin::Login))
}

/// The `auth_required` error when neither `IMIFY_TOKEN` nor a login for `host` exists. The
/// message names the places that were searched.
fn not_logged_in(host: &str, searched: &[Location]) -> CliError {
    let searched: Vec<String> = searched.iter().map(Location::to_string).collect();
    CliError::new(
        ErrorCode::AuthRequired,
        format!(
            "{} is not set, and no login for {host} is stored in {}.",
            imify::auth::EnvToken::VARIABLE,
            searched.join(" or ")
        ),
    )
    .with_hint(AUTH_REQUIRED_HINT)
}

/// The message of a passing `token` check for a stored login.
fn stored_login_message(host: &str, location: &Location, has_refresh_token: bool) -> String {
    let refresh = if has_refresh_token { "with" } else { "without" };
    format!("A login for {host} is stored in {location}, {refresh} a refresh token.")
}

/// Sets the hint of an `auth_rejected` failure as [`TokenOrigin::adjust`] does for the
/// login of `host`. Other checks are unchanged.
async fn with_origin_hint(
    check: Check,
    origin: TokenOrigin,
    env: &Environment,
    host: &str,
) -> Check {
    if check.code != Some(ErrorCode::AuthRejected) {
        return check;
    }
    let error = CliError::new(ErrorCode::AuthRejected, check.message.clone());
    let adjusted = origin.adjust(error, LoginLookup::new(env, host)).await;
    Check {
        hint: adjusted.hint,
        ..check
    }
}

async fn check_account(
    env: &Environment,
    api_url: &ResolvedApiUrl,
    host: &str,
    source: AuthSource,
    origin: TokenOrigin,
    report: &mut Report,
) {
    let result = match client_builder(&api_url.to_string())
        .token_source(source)
        .build()
    {
        Ok(client) => client.me().await.map_err(CliError::from),
        Err(error) => Err(api_url.invalid(&error)),
    };
    let check = match result {
        Ok(user) => {
            let check = Check::ok("account", format!("{} ({}).", user.name, user.role));
            report.user = Some(user);
            check
        }
        Err(error) => {
            let check = Check::fail("account", error);
            with_origin_hint(check, origin, env, host).await
        }
    };
    report.push(check);
}

impl Render for Report {
    fn kind(&self) -> &'static str {
        "doctor_report"
    }

    fn json(&self) -> serde_json::Value {
        let checks: Vec<serde_json::Value> = self
            .checks
            .iter()
            .map(|check| {
                serde_json::json!({
                    "name": check.name,
                    "status": check.status.as_str(),
                    "message": check.message,
                    "code": check.code,
                    "hint": check.hint,
                })
            })
            .collect();
        serde_json::json!({
            "cli_version": env!("CARGO_PKG_VERSION"),
            "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
            "api_url": self.api_url,
            "api_url_source": self.api_url_source.map(Source::as_str),
            "latency_ms": self.latency_ms,
            "clock_skew_ms": self.clock_skew_ms,
            "readiness": self.readiness.as_ref().map(|readiness| serde_json::json!({
                "status": readiness.status,
                "db": readiness.db,
                "storage": readiness.storage,
            })),
            "token_source": self.token_source,
            "token_expires_at": self.token_expires_at.map(rfc3339),
            "user": self.user.as_ref().map(|user| serde_json::json!({
                "subject": user.subject,
                "name": user.name,
                "role": user.role,
            })),
            "credential_store": self.credential_store.map(StoreChoice::as_str),
            "config_dir": self.config_dir.as_ref().map(|dir| dir.to_string_lossy()),
            "login": self.login.as_ref().map(|login| serde_json::json!({
                "storage": login.location.storage(),
                "has_refresh_token": login.has_refresh_token,
            })),
            "oidc": self.oidc.as_ref().map(|settings| serde_json::json!({
                "issuer": settings.issuer,
                "client_id": settings.client_id,
                "issuer_https": issuer_https(&settings.issuer),
            })),
            "checks": checks,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "imify-cli {} ({} {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        )?;
        writeln!(w)?;
        for check in &self.checks {
            writeln!(
                w,
                "{:<6}{:<14}{}",
                check.status.as_str(),
                check.name,
                printable(&check.message)
            )?;
            if let (Status::Fail | Status::Warn, Some(hint)) = (check.status, &check.hint) {
                writeln!(w, "{:<20}hint: {}", "", printable(hint))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn expired_stored_login_without_a_refresh_token_fails_with_auth_expired_and_the_login_hint() {
        let check = login_expiry_check(at(1_792_152_000), false, at(1_792_152_001));

        assert_eq!(
            (
                check.status,
                check.code,
                check.message.as_str(),
                check.hint.as_deref()
            ),
            (
                Status::Fail,
                Some(ErrorCode::AuthExpired),
                "The login expired at 2026-10-16T12:00:00Z and has no refresh token.",
                Some(LOGIN_HINT)
            )
        );
    }

    #[test]
    fn expired_stored_login_with_a_refresh_token_passes() {
        let check = login_expiry_check(at(1_792_152_000), true, at(1_792_152_001));

        assert_eq!(
            (check.status, check.message.as_str()),
            (
                Status::Ok,
                "Expired at 2026-10-16T12:00:00Z. The refresh token renews it."
            )
        );
    }

    #[test]
    fn stored_login_expiring_within_5_min_without_a_refresh_token_warns_with_the_login_hint() {
        let check = login_expiry_check(at(1_792_152_300), false, at(1_792_152_000));

        assert_eq!(
            (check.status, check.hint.as_deref()),
            (Status::Warn, Some(LOGIN_HINT))
        );
    }

    #[test]
    fn stored_login_expiring_within_5_min_with_a_refresh_token_passes() {
        let check = login_expiry_check(at(1_792_152_300), true, at(1_792_152_000));

        assert_eq!(
            (check.status, check.message.as_str()),
            (
                Status::Ok,
                "Expires at 2026-10-16T12:05:00Z, in less than 5 min. The refresh token renews it."
            )
        );
    }

    #[test]
    fn stored_login_expiring_later_than_5_min_passes() {
        let check = login_expiry_check(at(1_792_152_301), false, at(1_792_152_000));

        assert_eq!(
            (check.status, check.message.as_str()),
            (Status::Ok, "Expires at 2026-10-16T12:05:01Z.")
        );
    }

    #[test]
    fn https_issuer_passes_oidc() {
        let check = oidc_check(&OidcSettings::new("https://login.example.com", "client-1"));

        assert_eq!(
            (check.status, check.message.as_str()),
            (
                Status::Ok,
                "Issuer https://login.example.com, client client-1."
            )
        );
    }

    #[test]
    fn http_issuer_on_a_loopback_host_passes_oidc() {
        for issuer in [
            "http://localhost:8080",
            "http://127.0.0.1:8080/realms/imify",
            "http://[::1]:8080",
        ] {
            let check = oidc_check(&OidcSettings::new(issuer, "client-1"));

            assert_eq!(check.status, Status::Ok, "{issuer}");
        }
    }

    #[test]
    fn http_issuer_on_another_host_warns_on_oidc() {
        let check = oidc_check(&OidcSettings::new("http://login.example.com", "client-1"));

        assert_eq!(
            (check.status, check.code, check.hint.as_deref()),
            (
                Status::Warn,
                None,
                Some("Set `IMIFY_OIDC_ISSUER` to an `https` issuer.")
            )
        );
    }

    #[test]
    fn http_issuer_with_a_loopback_name_in_the_user_info_warns_on_oidc() {
        let check = oidc_check(&OidcSettings::new(
            "http://localhost@login.example.com",
            "client-1",
        ));

        assert_eq!(check.status, Status::Warn);
    }

    #[test]
    fn clock_skew_of_60_s_is_ok() {
        assert_eq!(clock_check(60_000).status, Status::Ok);
        assert_eq!(clock_check(-60_000).status, Status::Ok);
    }

    #[test]
    fn local_clock_61_s_behind_the_server_is_a_warning() {
        let check = clock_check(61_000);

        assert_eq!(
            (check.status, check.message.as_str()),
            (Status::Warn, "The local clock is 61 s behind the server.")
        );
    }

    #[test]
    fn local_clock_61_s_ahead_of_the_server_is_a_warning() {
        let check = clock_check(-61_000);

        assert_eq!(
            (check.status, check.message.as_str()),
            (Status::Warn, "The local clock is 61 s ahead of the server.")
        );
    }

    #[test]
    fn token_expired_in_the_past_fails_with_auth_rejected() {
        let check = expiry_check(at(1_792_152_000), at(1_792_152_001));

        assert_eq!(
            (check.status, check.code, check.message.as_str()),
            (
                Status::Fail,
                Some(ErrorCode::AuthRejected),
                "The token expired at 2026-10-16T12:00:00Z."
            )
        );
    }

    #[test]
    fn token_expiring_now_fails() {
        let check = expiry_check(at(1_792_152_000), at(1_792_152_000));

        assert_eq!(check.status, Status::Fail);
    }

    #[test]
    fn token_expiring_within_5_min_is_a_warning() {
        let check = expiry_check(at(1_792_152_300), at(1_792_152_000));

        assert_eq!(check.status, Status::Warn);
    }

    #[test]
    fn token_expiring_later_than_5_min_is_ok() {
        let check = expiry_check(at(1_792_152_301), at(1_792_152_000));

        assert_eq!(
            (check.status, check.message.as_str()),
            (Status::Ok, "Expires at 2026-10-16T12:05:01Z.")
        );
    }

    #[test]
    fn outcome_without_a_failure_is_ok_despite_warnings() {
        let checks = [
            Check::ok("api_url", "https://api.example.com"),
            Check::warn(
                "clock",
                "The local clock is 95 s behind the server.",
                "Sync.",
            ),
            Check::skip("token_expiry", "Skipped."),
        ];

        assert_eq!(outcome(&checks), Ok(()));
    }

    #[test]
    fn outcome_carries_the_code_and_hint_of_the_first_failing_check_and_names_all() {
        let checks = [
            Check::ok("api_url", "https://api.example.com"),
            Check::fail(
                "token",
                CliError::new(ErrorCode::AuthRequired, "IMIFY_TOKEN is not set.")
                    .with_hint("Set it."),
            ),
            Check::fail(
                "account",
                CliError::new(ErrorCode::Network, "Connection refused."),
            ),
        ];

        assert_eq!(
            outcome(&checks),
            Err(
                CliError::new(ErrorCode::AuthRequired, "2 checks failed: token, account.")
                    .with_hint("Set it.")
            )
        );
    }

    #[test]
    fn outcome_with_one_failure_says_1_check() {
        let checks = [Check::fail(
            "health",
            CliError::new(ErrorCode::Server, "Internal Server Error."),
        )];

        assert_eq!(
            outcome(&checks).unwrap_err().message,
            "1 check failed: health."
        );
    }

    #[test]
    fn signed_millis_is_negative_when_the_first_time_is_earlier() {
        assert_eq!(signed_millis(at(100), at(195)), -95_000);
        assert_eq!(signed_millis(at(195), at(100)), 95_000);
    }
}
