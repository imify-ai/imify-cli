# 0005. Host discovery and profiles

Status: Accepted
Date: 2026-10-16

## Context
The hosted service is the default target. Developers also target other stages. Requiring users to copy identity-provider details into a config file couples every user's setup to server configuration. Each host already serves an unauthenticated `/config.json` for its web app, with the identity issuer as `authority` and the web client's id as `clientId`.

## Decision
- The default host is compiled in. A user needs no configuration for the hosted service.
- The CLI discovers the identity issuer, client id and API version from an unauthenticated `GET /api/v1/client-config` on the host, and caches the result per profile.
- Until `client-config` exists, the SDK reads `authority` and `clientId` from `GET /config.json` on the host (`Client::oidc_settings`) and ignores other keys. In the CLI, `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID` override them; when both are set, `config.json` is not fetched.
- The issuer's endpoints come from its OIDC discovery document, `<issuer>/.well-known/openid-configuration`. The document's `issuer` must equal the configured issuer.
- Credentials belong to the host: a login is stored under the API URL it was made for.
- Named profiles in the config file select other hosts: `--profile`, `IMIFY_PROFILE`. `IMIFY_API_URL` overrides the host.

## Consequences
Changing the identity provider is a server-side change. A host that serves neither `client-config` nor `config.json` needs the two override variables.
