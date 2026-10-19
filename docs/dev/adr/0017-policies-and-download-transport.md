# 0017. Policies command group and download transport

Status: Accepted
Date: 2026-10-19

## Context
Users train policies on the platform and load them with LeRobot's `--policy.path`. The API does not model policies. It models training runs under `/api/v1/training-runs`. A run has a `policy` field with the architecture, a `status`, a final artifact and checkpoints. For each artifact and checkpoint, the API returns a manifest: one presigned storage URL per file, with the file's relative path and size. The manifest carries no hash and no expiry time.

[0002](0002-noun-verb-command-grammar.md) states that the API's resource names define the command nouns. The API will expose a policy resource. Until it does, the resource name and the domain term differ.

The training routes are not in the vendored OpenAPI document yet, so [0015](0015-generated-sdk-api-layer.md) cannot generate their types.

## Decision

### Vocabulary

| Term | Meaning |
|---|---|
| Policy | The trained result: maps observations to actions, loadable with LeRobot's `--policy.path` |
| Policy type | The architecture, such as `act`, `groot_n1.7` or `pi0.5` |
| Checkpoint | A policy's state at one training step, also loadable |
| Training | How a policy comes into being; a state of the policy |

### Command group
- Commands for policies are `imify policies <verb>`.
- The noun `policies` over the training-run routes is a temporary exception to the rule of [0002](0002-noun-verb-command-grammar.md). The exception ends when the API exposes a policy resource; the noun then names that resource. [0002](0002-noun-verb-command-grammar.md) is unchanged.

### Mapping onto the training-run API
- Policy id = training run id.
- `policy_type` = the run's `policy` field. An unknown value passes through as is.
- `state` = the run's `status`: `queued` → `queued`, `running` → `training`, `succeeded` → `ready`, `failed` → `failed`. An unknown value passes through as is.
- The policy = the run's final artifact.
- The CLI's output contract ([0003](0003-output-contract.md)) uses the policy vocabulary only. An API rename to a policy resource changes the SDK's wire layer, not the CLI's `--json` output.
- The JSON Schemas declare `state` and `policy_type` as strings and document the known values. They declare no closed `enum`, so a new server value is not a breaking change of the output contract.
- Until the vendored document contains the training routes, the SDK's training-run wire types are hand-written and crate-private. Afterwards, the generated types are public SDK API under [0015](0015-generated-sdk-api-layer.md).
- The policy mapping and the download engine are hand-written, in addition to the parts that [0015](0015-generated-sdk-api-layer.md) lists.

### Download transport
- The SDK fetches the manifest of an artifact or a checkpoint and downloads its files with bounded concurrency, default 4, as in [0006](0006-upload-transport.md).
- Each file downloads into `<file>.part`. After the transfer, its size is checked against the manifest and the file is renamed into place. A SHA-256 check follows once the manifest carries a hash.
- A re-run skips files that exist with the size from the manifest. There is no journal: the target directory is the state.
- Network errors and 5xx responses from storage retry with the exponential backoff and jitter of [0006](0006-upload-transport.md).
- A 403 from storage means the presigned URLs expired. The SDK fetches a new manifest once and continues. A second 403 for the same file is an error.
- Requests to presigned URLs never carry the bearer token and follow no redirect to another origin.
- A `relative_path` that is absolute, empty or contains `..` is rejected before any file is written.
- The SDK does not unpack archives.
- There is one transport, so the download has no `Transport` trait.

## Consequences
The CLI speaks the domain vocabulary before the API does. When the API exposes a policy resource, the SDK's wire layer and its public generated types change; the CLI's commands and `--json` output stay as they are.

A file with the expected size but wrong content is skipped on a re-run until the manifest carries a SHA-256. Files in the target directory that are not in the manifest are left alone. Storage that offers no downloads, which the API reports with 422, has no fallback through the API.

Tests serve the presigned URLs from a second mock server with a different origin, so the missing bearer token and the refused cross-origin redirect are tested as stated ([0010](0010-mock-only-testing.md)).
