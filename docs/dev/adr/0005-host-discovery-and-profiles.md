# 0005. Host discovery and profiles

Status: Accepted
Date: 2026-10-16
Amended: 2026-10-19

## Context
The hosted service is the default target. Developers also target other stages. Requiring users to copy identity-provider details into a config file couples every user's setup to server configuration. Each host serves an unauthenticated `/config.json` for its web app, with the identity issuer as `authority` and the web client's id as `clientId`. Hosts also publish OAuth 2.0 Protected Resource Metadata (RFC 9728) for the versioned API. The identity provider supports neither dynamic client registration nor client id metadata documents, so the metadata carries the CLI's client id in an extension field.

## Decision
- The default host is compiled in. A user needs no configuration for the hosted service.
- The SDK reads the issuer and client id from the protected resource metadata of the API (`Client::oidc_settings`): an unauthenticated `GET` of `/.well-known/oauth-protected-resource` followed by the base URL path and `api/v1`, on the API origin (RFC 9728, section 3.1). The issuer is the first entry of `authorization_servers`, the client id is `imify_cli_client_id`. `resource` must equal the base URL followed by `api/v1`, ignoring a trailing slash (RFC 9728, section 3.3).
- A 404, or a 403 without a JSON body, means the host publishes no metadata. The SDK then reads `authority` and `clientId` from `GET /config.json` on the host and ignores other keys. Neither status is retried. Every other failure of the metadata request is returned without the fallback. The fallback stays until every host serves the metadata.
- The `resource_metadata` parameter of a `WWW-Authenticate` challenge is not used; the well-known URL is fixed.
- In the CLI, `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID` override the published settings; when both are set, neither the metadata nor `config.json` is fetched.
- The issuer's endpoints come from its OIDC discovery document, `<issuer>/.well-known/openid-configuration`. The document's `issuer` must equal the configured issuer.
- Credentials belong to the host: a login is stored under the API URL it was made for.
- Named profiles in the config file select other hosts: `--profile`, `IMIFY_PROFILE`. `IMIFY_API_URL` overrides the host.

## Consequences
Changing the identity provider is a server-side change. A host that serves neither the protected resource metadata nor `config.json` needs the two override variables. Removing the `config.json` fallback is a later change.
