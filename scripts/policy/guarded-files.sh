#!/usr/bin/env bash
# Reads file paths from stdin and prints those matching a pattern in .github/guarded-paths.
# Exits 1 when at least one path is guarded, 0 otherwise.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
patterns=()
while IFS= read -r line; do
  [[ -z "$line" || "$line" == \#* ]] && continue
  patterns+=("$line")
done <"$root/.github/guarded-paths"

found=0
while IFS= read -r path; do
  for pattern in "${patterns[@]}"; do
    # shellcheck disable=SC2053 # the right-hand side is a glob on purpose
    if [[ "$path" == $pattern ]]; then
      echo "$path"
      found=1
      break
    fi
  done
done

exit "$found"
