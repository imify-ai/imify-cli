# 0007. Dataset identity is derived by the server

Status: Accepted
Date: 2026-10-16

## Context
On the imify platform a dataset is not created explicitly. When an upload completes, the server assigns its episodes to the dataset identified by task, robot embodiment and format, creating the dataset if none matches. Hugging Face, by contrast, uses explicitly named repositories.

## Decision
There is no `imify datasets create`. `imify datasets upload` reports whether it created a dataset or appended episodes to an existing one. `--dry-run` resolves the target dataset without uploading.

## Consequences
The CLI mirrors the platform's domain model and adds none of its own. If the platform introduces named datasets or a different aggregate, the CLI follows the API.
