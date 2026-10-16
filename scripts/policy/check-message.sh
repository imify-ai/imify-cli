#!/usr/bin/env bash
# Validates commit messages, PR titles and PR bodies against the contribution policy.
#
#   check-message.sh attribution <file>   no co-author or attribution lines
#   check-message.sh conventional <file>  first line is a conventional commit header
#
# Exits 1 with a message on stderr when the input violates the policy.
set -euo pipefail

readonly CONVENTIONAL='^(feat|fix|docs|refactor|perf|test|build|ci|chore|revert)(\([a-z0-9-]+\))?!?: [^ ].*$'
readonly ATTRIBUTION='(^[[:space:]]*co-authored-by:)|(^[^[:alnum:]]*generated (with|by) \[)'

usage() {
  echo "usage: $0 attribution|conventional <file>" >&2
  exit 2
}

[[ $# -eq 2 ]] || usage
mode=$1
file=$2

case "$mode" in
  attribution)
    if grep -Eiq "$ATTRIBUTION" "$file" || grep -Fq $'\xf0\x9f\xa4\x96' "$file"; then
      echo "policy: remove co-author and attribution lines:" >&2
      grep -E -i -n "$ATTRIBUTION" "$file" >&2 || true
      exit 1
    fi
    ;;
  conventional)
    header=$(head -n 1 "$file")
    if ! grep -Eq "$CONVENTIONAL" <<<"$header"; then
      echo "policy: '$header' is not a conventional commit header, e.g. 'feat(datasets): add upload --resume'" >&2
      exit 1
    fi
    ;;
  *) usage ;;
esac
