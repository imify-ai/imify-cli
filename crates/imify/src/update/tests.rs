use std::time::{Duration, Instant};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const LATEST: &str = "/repos/imify-ai/imify-cli/releases/latest";

fn client(server: &MockServer) -> ReleaseClient {
    ReleaseClient::builder(ReleaseSource::imify_cli())
        .api_url(&server.uri())
        .download_url(&server.uri())
        .build()
        .unwrap()
}

fn release_body(tag: &str) -> serde_json::Value {
    json!({
        "tag_name": tag,
        "name": tag,
        "draft": false,
        "prerelease": false,
        "assets": [{"name": "imify-cli-installer.sh"}]
    })
}

async fn api_answering(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(LATEST))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

async fn latest_error(response: ResponseTemplate) -> Error {
    let server = api_answering(response).await;
    client(&server).latest().await.unwrap_err()
}

#[tokio::test]
async fn latest_returns_the_tag_and_its_version_without_the_leading_v() {
    let server =
        api_answering(ResponseTemplate::new(200).set_body_json(release_body("v0.2.0"))).await;

    let release = client(&server).latest().await.unwrap();

    assert_eq!(
        (release.tag.as_str(), release.version),
        ("v0.2.0", Version::new(0, 2, 0))
    );
}

#[tokio::test]
async fn latest_sends_the_user_agent_and_the_github_accept_header_only() {
    let server =
        api_answering(ResponseTemplate::new(200).set_body_json(release_body("v0.2.0"))).await;

    client(&server).latest().await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let headers = &requests[0].headers;
    assert_eq!(
        (
            headers.get("user-agent").and_then(|v| v.to_str().ok()),
            headers.get("accept").and_then(|v| v.to_str().ok()),
            headers.contains_key("authorization"),
            headers.contains_key("cookie"),
            requests[0].url.query(),
        ),
        (
            Some(format!("imify-cli/{}", crate::VERSION).as_str()),
            Some("application/vnd.github+json"),
            false,
            false,
            None,
        )
    );
}

#[tokio::test]
async fn latest_with_a_tag_that_is_not_semver_is_an_unexpected_response() {
    let error =
        latest_error(ResponseTemplate::new(200).set_body_json(release_body("nightly"))).await;

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn latest_with_a_body_that_is_not_a_release_is_an_unexpected_response() {
    let error =
        latest_error(ResponseTemplate::new(200).set_body_json(json!({"message": "hi"}))).await;

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn latest_on_403_and_429_is_rate_limited() {
    let forbidden = latest_error(
        ResponseTemplate::new(403).set_body_json(json!({"message": "API rate limit exceeded"})),
    )
    .await;
    let too_many =
        latest_error(ResponseTemplate::new(429).insert_header("retry-after", "60")).await;

    assert!(
        matches!(forbidden, Error::RateLimited { retry_after: None })
            && matches!(too_many, Error::RateLimited { retry_after: Some(delay) } if delay == Duration::from_secs(60)),
        "{forbidden:?} {too_many:?}"
    );
}

#[tokio::test]
async fn latest_on_404_is_not_found() {
    let error = latest_error(ResponseTemplate::new(404)).await;

    assert!(matches!(error, Error::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn latest_on_503_is_a_server_error() {
    let error = latest_error(ResponseTemplate::new(503)).await;

    assert!(
        matches!(error, Error::Server { status: 503, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn latest_on_another_status_is_an_unexpected_response() {
    let error = latest_error(ResponseTemplate::new(418)).await;

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 418, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn latest_slower_than_the_timeout_is_a_network_error_within_the_timeout() {
    let server = api_answering(
        ResponseTemplate::new(200)
            .set_body_json(release_body("v0.2.0"))
            .set_delay(Duration::from_secs(10)),
    )
    .await;
    let client = ReleaseClient::builder(ReleaseSource::imify_cli())
        .api_url(&server.uri())
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let start = Instant::now();

    let error = client.latest().await.unwrap_err();

    assert!(matches!(error, Error::Network { .. }), "{error:?}");
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn latest_does_not_follow_a_redirect_to_another_origin() {
    let other =
        api_answering(ResponseTemplate::new(200).set_body_json(release_body("v9.0.0"))).await;
    let server = api_answering(
        ResponseTemplate::new(307).insert_header("location", format!("{}{LATEST}", other.uri())),
    )
    .await;

    let error = client(&server).latest().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 307, .. }),
        "{error:?}"
    );
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn latest_with_a_body_over_1_mib_is_an_unexpected_response() {
    let error =
        latest_error(ResponseTemplate::new(200).set_body_string("x".repeat(1024 * 1024 + 1))).await;

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[test]
fn an_http_api_url_on_a_host_other_than_loopback_is_a_config_error() {
    let error = ReleaseClient::builder(ReleaseSource::imify_cli())
        .api_url("http://api.example.com")
        .build()
        .unwrap_err();

    assert!(matches!(error, Error::Config { .. }), "{error:?}");
}

#[test]
fn an_http_download_url_on_a_host_other_than_loopback_is_a_config_error() {
    let error = ReleaseClient::builder(ReleaseSource::imify_cli())
        .download_url("http://github.example.com")
        .build()
        .unwrap_err();

    assert!(matches!(error, Error::Config { .. }), "{error:?}");
}

#[test]
fn the_receipt_of_imify_cli_is_below_the_config_home() {
    let path = ReleaseSource::imify_cli().receipt_path(Path::new("/home/ada/.config"));

    assert_eq!(
        path,
        Path::new("/home/ada/.config/imify-cli/imify-cli-receipt.json")
    );
}
/// A fake install below a temporary directory: `prefix/bin/imify` and a receipt.
struct FakeInstall {
    dir: tempfile::TempDir,
}

impl FakeInstall {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("prefix/bin")).unwrap();
        std::fs::write(dir.path().join("prefix/bin/imify"), "binary").unwrap();
        Self { dir }
    }

    fn prefix(&self) -> PathBuf {
        self.dir.path().join("prefix")
    }

    fn executable(&self) -> PathBuf {
        self.prefix().join("bin/imify")
    }

    fn receipt(&self) -> PathBuf {
        ReleaseSource::imify_cli().receipt_path(&self.dir.path().join("config"))
    }

    /// Writes a receipt as the 0.1.0 installer writes it, changed by `edit`.
    fn write_receipt(&self, edit: impl FnOnce(&mut serde_json::Value)) {
        let mut receipt = json!({
            "binaries": ["imify"],
            "binary_aliases": {},
            "cdylibs": [],
            "cstaticlibs": [],
            "install_layout": "cargo-home",
            "install_prefix": self.prefix(),
            "modify_path": true,
            "provider": {"source": "cargo-dist", "version": "0.33.0"},
            "source": {
                "app_name": "imify-cli",
                "name": "imify-cli",
                "owner": "imify-ai",
                "release_type": "github"
            },
            "version": "0.1.0"
        });
        edit(&mut receipt);
        self.write_receipt_text(&receipt.to_string());
    }

    fn write_receipt_text(&self, text: &str) {
        let receipt = self.receipt();
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(receipt, text).unwrap();
    }

    fn installation(&self) -> Result<Option<Installation>, Error> {
        Installation::from_receipt(
            &self.receipt(),
            &self.executable(),
            &ReleaseSource::imify_cli(),
        )
    }

    /// The file the fake installer writes its environment to.
    fn installer_env_file(&self) -> PathBuf {
        self.dir.path().join("installer-env")
    }

    /// A shell installer that writes its environment to [`Self::installer_env_file`], prints
    /// one line to stdout and one to stderr, and exits with `status`.
    fn installer_script(&self, status: i32) -> String {
        format!(
            "#!/bin/sh\nenv > '{}'\necho 'installing imify-cli'\necho 'warning: none' >&2\nexit {status}\n",
            self.installer_env_file().display()
        )
    }

    /// The environment the fake installer saw, as `NAME=value` lines; empty when it did not
    /// run.
    fn installer_env(&self) -> Vec<String> {
        std::fs::read_to_string(self.installer_env_file())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

const INSTALLER: &str = "/imify-ai/imify-cli/releases/download/v0.2.0/imify-cli-installer.sh";

fn release() -> Release {
    Release {
        tag: "v0.2.0".to_owned(),
        version: Version::new(0, 2, 0),
    }
}

async fn serving_installer(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(INSTALLER))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[test]
fn from_receipt_without_a_receipt_is_none() {
    let install = FakeInstall::new();

    assert!(install.installation().unwrap().is_none());
}

#[test]
fn from_receipt_returns_the_install_prefix_when_the_executable_is_in_its_bin_directory() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let installation = install.installation().unwrap().unwrap();

    assert_eq!(installation.install_prefix(), install.prefix());
}

#[test]
fn from_receipt_is_none_when_the_prefix_does_not_contain_the_executable() {
    let install = FakeInstall::new();
    install.write_receipt(|receipt| receipt["install_prefix"] = json!("/opt/elsewhere"));

    assert!(install.installation().unwrap().is_none());
}

#[test]
fn from_receipt_is_none_when_the_receipt_lists_another_binary() {
    let install = FakeInstall::new();
    install.write_receipt(|receipt| receipt["binaries"] = json!(["other"]));

    assert!(install.installation().unwrap().is_none());
}

#[test]
fn from_receipt_is_none_for_a_receipt_of_another_repository() {
    let install = FakeInstall::new();
    install.write_receipt(|receipt| receipt["source"]["owner"] = json!("someone-else"));

    assert!(install.installation().unwrap().is_none());
}

#[test]
fn from_receipt_of_invalid_json_is_a_self_update_error() {
    let install = FakeInstall::new();
    install.write_receipt_text("{not json");

    let error = install.installation().unwrap_err();

    assert!(
        matches!(
            error,
            Error::SelfUpdate {
                installer_output: None,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn from_receipt_with_a_flat_layout_is_a_self_update_error() {
    let install = FakeInstall::new();
    install.write_receipt(|receipt| receipt["install_layout"] = json!("flat"));

    let error = install.installation().unwrap_err();

    assert!(matches!(error, Error::SelfUpdate { .. }), "{error:?}");
}

#[test]
fn from_receipt_accepts_the_unspecified_layout_of_older_installers() {
    let install = FakeInstall::new();
    install.write_receipt(|receipt| receipt["install_layout"] = json!("unspecified"));

    assert!(install.installation().unwrap().is_some());
}

async fn install_with(
    install: &FakeInstall,
    response: ResponseTemplate,
) -> (MockServer, Result<String, Error>) {
    let server = serving_installer(response).await;
    let installation = install.installation().unwrap().unwrap();
    let result = client(&server).install(&installation, &release()).await;
    (server, result)
}

fn script_response(install: &FakeInstall, status: i32) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(install.installer_script(status))
}

#[tokio::test]
async fn install_runs_the_installer_of_the_release_for_the_receipts_prefix() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (_server, result) = install_with(&install, script_response(&install, 0)).await;

    result.unwrap();
    let expected = format!("IMIFY_CLI_INSTALL_DIR={}", install.prefix().display());
    assert!(
        install.installer_env().contains(&expected),
        "{:?}",
        install.installer_env()
    );
}

#[tokio::test]
async fn install_returns_the_installer_output() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (_server, result) = install_with(&install, script_response(&install, 0)).await;

    assert_eq!(result.unwrap(), "installing imify-cli\nwarning: none\n");
}

#[tokio::test]
async fn install_keeps_the_path_unchanged_when_the_receipt_says_so() {
    let install = FakeInstall::new();
    install.write_receipt(|receipt| receipt["modify_path"] = json!(false));

    let (_server, result) = install_with(&install, script_response(&install, 0)).await;

    result.unwrap();
    assert!(
        install
            .installer_env()
            .contains(&"IMIFY_CLI_NO_MODIFY_PATH=1".to_owned())
    );
}

#[tokio::test]
async fn install_lets_the_installer_change_the_path_when_the_receipt_says_so() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (_server, result) = install_with(&install, script_response(&install, 0)).await;

    result.unwrap();
    assert!(
        !install
            .installer_env()
            .iter()
            .any(|line| line.starts_with("IMIFY_CLI_NO_MODIFY_PATH="))
    );
}

#[tokio::test]
async fn install_from_another_download_url_points_the_installer_at_it() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (server, result) = install_with(&install, script_response(&install, 0)).await;

    result.unwrap();
    let expected = format!("IMIFY_CLI_INSTALLER_GHE_BASE_URL={}", server.uri());
    assert!(install.installer_env().contains(&expected));
}

#[tokio::test]
async fn install_with_a_failing_installer_is_a_self_update_error_with_its_output() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (_server, result) = install_with(&install, script_response(&install, 3)).await;

    let error = result.unwrap_err();
    assert!(
        matches!(
            &error,
            Error::SelfUpdate { installer_output: Some(output), .. }
                if output == "installing imify-cli\nwarning: none\n"
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn install_of_a_release_without_an_installer_is_a_self_update_error() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (_server, result) = install_with(&install, ResponseTemplate::new(404)).await;

    let error = result.unwrap_err();
    assert!(
        matches!(
            error,
            Error::SelfUpdate {
                installer_output: None,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn install_does_not_run_a_download_that_is_not_a_shell_script() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});

    let (_server, result) = install_with(
        &install,
        ResponseTemplate::new(200).set_body_string("<html>sign in</html>"),
    )
    .await;

    let error = result.unwrap_err();
    assert!(
        matches!(
            error,
            Error::SelfUpdate {
                installer_output: None,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn install_follows_a_302_redirect_within_the_download_origin() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});
    let server = serving_installer(
        ResponseTemplate::new(302).insert_header("location", "/assets/installer.sh"),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/assets/installer.sh"))
        .respond_with(script_response(&install, 0))
        .mount(&server)
        .await;
    let installation = install.installation().unwrap().unwrap();

    client(&server)
        .install(&installation, &release())
        .await
        .unwrap();

    assert_ne!(install.installer_env(), Vec::<String>::new());
}

#[tokio::test]
async fn install_does_not_follow_a_redirect_to_plain_http_on_another_origin() {
    let install = FakeInstall::new();
    install.write_receipt(|_| {});
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(script_response(&install, 0))
        .mount(&other)
        .await;

    let (_server, result) = install_with(
        &install,
        ResponseTemplate::new(302)
            .insert_header("location", format!("{}/installer.sh", other.uri())),
    )
    .await;

    assert!(result.is_err());
    assert!(other.received_requests().await.unwrap().is_empty());
    assert_eq!(install.installer_env(), Vec::<String>::new());
}
