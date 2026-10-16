# 0004. Browser login, environment token, platform tokens later

Status: Accepted
Date: 2026-10-16

## Context
`gh`, `glab` and `gcloud` share one model: browser sign-in for people, a long-lived token issued by the platform for machines, and one environment variable that overrides both. The imify identity provider offers no device-code flow, and the platform does not yet issue its own tokens. The API accepts the ID token of the host's web client as bearer token. That client is public and has no client secret.

## Decision
- `imify auth login` uses the OpenID Connect authorization code flow with PKCE (`S256`), `state` and `nonce`, and the scopes `openid email profile`.
- The redirect URI is `http://localhost:<port>/callback` on the fixed ports 28753, 28754 and 28755, tried in order. The listener binds `127.0.0.1` and, when available, `::1`. The ports are registered with the identity provider client; changing them needs a release and a new registration.
- `--no-browser` prints the URL and accepts the pasted redirect URL. It uses the redirect URI on port 28753 and binds nothing.
- The bearer token is the ID token. Its expiry is the `exp` claim. The SDK checks `iss`, `aud` (contains the client id; with several entries, `azp` equals the client id), `exp` (60 s leeway, at most one day ahead), `sub` (unchanged by a refresh) and, on the code exchange, `nonce`. It does not verify the signature: the token comes directly from the token endpoint over TLS (OIDC Core 1.0, section 3.1.3.7). The API verifies it.
- The token source refreshes the ID token when less than 60 s remain before `exp`, and saves the result. A refresh that answers `invalid_grant` ends the login, unless another process has saved a fresh login in the meantime.
- The API URL, the issuer and every endpoint from discovery use `https`. `http` is allowed only for the hosts `localhost`, `127.0.0.1` and `::1`.
- Logout revokes the refresh token (RFC 7009) at the `revocation_endpoint` from discovery. Without one, it uses `/oauth2/revoke` on the token endpoint's origin, which already receives the refresh token. A 404 there means revocation is unsupported. Issued ID tokens stay valid until they expire.
- Credentials are keyed by host: one login per API URL. They are stored in the OS keychain, with a file fallback readable only by the user. `IMIFY_CREDENTIAL_STORE` (`auto`, `keychain`, `file`) selects the store; `auto` tries the keychain first.
- `IMIFY_TOKEN` overrides stored credentials.
- The SDK models credentials as a `TokenSource` trait and storage as a `CredentialStore` trait. Platform-issued API tokens become another `TokenSource` when the platform supports them.

## Consequences
People and agents on a signed-in machine work without extra setup. Unattended use (CI, robots) is limited to short-lived tokens passed via `IMIFY_TOKEN` until platform-issued tokens exist. Tokens never appear in logs, errors or verbose output. A login fails when all three redirect ports are taken; `--no-browser` works around it. Logout cannot invalidate an issued ID token.
