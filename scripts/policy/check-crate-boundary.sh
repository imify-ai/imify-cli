#!/usr/bin/env bash
# Fails when imify-cli depends directly on an HTTP client. All network access goes through
# the imify SDK crate.
set -euo pipefail

readonly FORBIDDEN='^(reqwest|hyper|hyper-util|ureq|isahc|surf|attohttpc|curl|h2|http) '

direct=$(cargo tree -p imify-cli -e normal --depth 1 --prefix none --format '{p}' | tail -n +2)
if violations=$(grep -E "$FORBIDDEN" <<<"$direct"); then
  echo "policy: imify-cli must not depend on an HTTP client directly; use the imify SDK:" >&2
  echo "$violations" >&2
  exit 1
fi
