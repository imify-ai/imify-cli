# 0013. Contribution policy

Status: Accepted
Date: 2026-10-16

## Context
The repository is public and licensed under Apache-2.0.

## Decision
- Development happens on GitHub: issues, pull requests, Actions.
- One branch per issue: `feature/issue-<id>-<slug>` or `fix/issue-<id>-<slug>`. PRs are squash-merged; the PR title is a conventional commit header.
- Commits and PR descriptions carry no co-author or attribution lines.
- No DCO and no CLA. Contributions are licensed under Apache-2.0, section 5.

## Consequences
History is linear and every commit on `main` maps to one PR and one issue. Relicensing would require a CLA, which this policy does not provide.
