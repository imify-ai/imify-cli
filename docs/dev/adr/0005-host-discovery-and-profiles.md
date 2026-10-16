# 0005. Host discovery and profiles

Status: Accepted
Date: 2026-10-16

## Context
The hosted service is the default target. Developers also target other stages. Requiring users to copy identity-provider details into a config file couples every user's setup to server configuration.

## Decision
- The default host is compiled in. A user needs no configuration for the hosted service.
- The CLI discovers the identity issuer, client id and API version from an unauthenticated `GET /api/v1/client-config` on the host, and caches the result per profile.
- Named profiles in the config file select other hosts: `--profile`, `IMIFY_PROFILE`. `IMIFY_API_URL` overrides the host.

## Consequences
Changing the identity provider is a server-side change. A host without the discovery endpoint is not supported.
