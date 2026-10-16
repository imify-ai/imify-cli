# 0004. Browser login, environment token, platform tokens later

Status: Accepted
Date: 2026-10-16

## Context
`gh`, `glab` and `gcloud` share one model: browser sign-in for people, a long-lived token issued by the platform for machines, and one environment variable that overrides both. The imify identity provider offers no device-code flow, and the platform does not yet issue its own tokens.

## Decision
- `imify login` uses the OAuth 2.0 authorization code flow with PKCE and a loopback redirect on `127.0.0.1`. `--no-browser` prints the URL and accepts the pasted redirect URL.
- Refresh tokens are stored in the OS keychain, with a file fallback readable only by the user. Access is refreshed silently.
- `IMIFY_TOKEN` overrides stored credentials.
- The SDK models credentials as a `TokenSource` trait. Platform-issued API tokens become another implementation when the platform supports them.

## Consequences
People and agents on a signed-in machine work without extra setup. Unattended use (CI, robots) is limited to short-lived tokens passed via `IMIFY_TOKEN` until platform-issued tokens exist. Tokens never appear in logs, errors or verbose output.
