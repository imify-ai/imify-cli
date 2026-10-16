//! `imify doctor`: checks the CLI configuration, the API and the access token.

use std::io::{self, Write};
use std::time::{Duration, Instant, SystemTime};

use imify::auth::TokenSource;
use imify::{Client, ClientBuilder, Readiness, RetryPolicy, User};

use super::auth::TokenOrigin;
use super::{api_url, invalid_api_url, write_result};
use crate::output::renderer::{Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::time::rfc3339;
use crate::output::{AUTH_REJECTED_HINT, AUTH_REQUIRED_HINT, CliError, Environment, ErrorCode};

/// Connect and read timeout of every request doctor sends.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Largest difference between the local clock and the server's `Date` that passes.
const CLOCK_SKEW_LIMIT_MS: i64 = 60_000;
/// A token that expires within this time gets a warning.
const EXPIRY_WARNING: Duration = Duration::from_secs(5 * 60);
/// Hosts that `http` may address without a warning.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

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
}

/// The `api_url` check of a URL the SDK accepted: `warn` for `http` to a host other than
/// the loopback host.
fn api_url_check(url: &str) -> Check {
    if sends_plain_http_off_host(url) {
        Check::warn(
            "api_url",
            format!("{url} uses plain http to a host other than localhost."),
            "Use an `https` URL in `IMIFY_API_URL`.",
        )
    } else {
        Check::ok("api_url", url)
    }
}

/// Returns `true` when `url` is an `http` URL whose host is not a loopback host.
fn sends_plain_http_off_host(url: &str) -> bool {
    let scheme = "http://";
    if !url
        .get(..scheme.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    {
        return false;
    }
    let authority = url[scheme.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host_and_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_and_port.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host_and_port.split(':').next().unwrap_or_default(),
    };
    !LOOPBACK_HOSTS
        .iter()
        .any(|loopback| host.eq_ignore_ascii_case(loopback))
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
    latency_ms: Option<u64>,
    clock_skew_ms: Option<i64>,
    readiness: Option<Readiness>,
    token_source: Option<&'static str>,
    token_expires_at: Option<SystemTime>,
    user: Option<User>,
    checks: Vec<Check>,
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
    let mut report = Report::default();

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

    let host = api.as_ref().map(|(url, _)| url.as_str());
    let token_source = check_token(env, host, &mut report).await;
    match (&api, token_source) {
        (_, None) => report.push(Check::skip_after("account", "token")),
        (Some((url, _)), Some(source)) if reachable.is_some() => {
            check_account(url, source, &mut report).await;
        }
        _ => report.push(Check::skip_after("account", "health")),
    }
    report
}

/// Runs the `api_url` check and returns the URL with a client without a token source.
fn check_api_url(env: &Environment, report: &mut Report) -> Option<(String, Client)> {
    let checked = api_url(env).and_then(|url| {
        let url = url.as_str().trim_end_matches('/').to_owned();
        client_builder(&url)
            .build()
            .map(|client| (url, client))
            .map_err(|error| invalid_api_url(&error))
    });
    match checked {
        Ok((url, client)) => {
            report.push(api_url_check(&url));
            report.api_url = Some(url.clone());
            Some((url, client))
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

/// Runs the `token` and `token_expiry` checks and returns the token source when it
/// supplies a token.
async fn check_token(
    env: &Environment,
    host: Option<&str>,
    report: &mut Report,
) -> Option<impl TokenSource + 'static> {
    let variable = imify::auth::EnvToken::VARIABLE;
    let resolved = match host {
        Some(host) => super::auth::token_source(env, &host_key_of(host)).await,
        None => super::auth::env_token_source(),
    };
    let (source, origin) = match resolved {
        Ok(Some((source, origin))) => (Ok(source), origin),
        Ok(None) => (
            Err(
                CliError::new(ErrorCode::AuthRequired, format!("{variable} is not set."))
                    .with_hint(AUTH_REQUIRED_HINT),
            ),
            TokenOrigin::Env,
        ),
        Err(error) => (Err(error), TokenOrigin::Env),
    };
    let token = match source {
        Ok(source) => source
            .token()
            .await
            .map(|token| (source, token))
            .map_err(CliError::from),
        Err(error) => Err(error),
    };
    let (source, token) = match token {
        Ok(found) => found,
        Err(error) => {
            report.push(Check::fail("token", error));
            report.push(Check::skip_after("token_expiry", "token"));
            return None;
        }
    };
    report.push(Check::ok(
        "token",
        match origin {
            TokenOrigin::Env => format!("{variable} is set."),
            TokenOrigin::Login => "A login is stored for the host.".to_owned(),
        },
    ));
    report.token_source = Some(match origin {
        TokenOrigin::Env => "env",
        TokenOrigin::Login => "stored",
    });
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
    report.push(expiry);
    Some(source)
}

/// The host key of an `api_url` the `api_url` check accepted.
fn host_key_of(api_url: &str) -> String {
    url::Url::parse(api_url).map_or_else(
        |_| api_url.to_owned(),
        |url| crate::credentials::host_key(&url),
    )
}

async fn check_account(url: &str, source: impl TokenSource + 'static, report: &mut Report) {
    let result = match client_builder(url).token_source(source).build() {
        Ok(client) => client.me().await.map_err(CliError::from),
        Err(error) => Err(invalid_api_url(&error)),
    };
    let check = match result {
        Ok(user) => {
            let check = Check::ok("account", format!("{} ({}).", user.name, user.role));
            report.user = Some(user);
            check
        }
        Err(error) => Check::fail("account", error),
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
    fn https_api_url_is_ok() {
        assert_eq!(api_url_check("https://api.example.com").status, Status::Ok);
    }

    #[test]
    fn http_api_url_on_a_loopback_host_is_ok() {
        for url in [
            "http://localhost:8000",
            "http://127.0.0.1:8000/api",
            "http://[::1]:8000",
            "HTTP://LOCALHOST",
        ] {
            assert_eq!(api_url_check(url).status, Status::Ok, "{url}");
        }
    }

    #[test]
    fn http_api_url_on_another_host_is_a_warning_with_a_hint() {
        let check = api_url_check("http://api.example.com");

        assert_eq!(check.status, Status::Warn);
        assert!(check.hint.is_some());
    }

    #[test]
    fn http_api_url_with_a_loopback_name_in_the_user_info_is_a_warning() {
        assert_eq!(
            api_url_check("http://localhost@api.example.com").status,
            Status::Warn
        );
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
