# 0010. Tests run against mocks only

Status: Accepted
Date: 2026-10-16

## Context
The platform's server is not available to public CI, and the CLI must be developed without real infrastructure or robot data.

## Decision
All automated tests run against mock servers (`wiremock`) and generated datasets. CLI end-to-end tests run the real binary (`assert_cmd`) against a mock server and pin output with snapshots (`insta`).

## Consequences
Behavior outside the OpenAPI contract, such as proxy timeouts or rate limits of the hosted service, is not covered by tests. It is verified manually before major releases.
