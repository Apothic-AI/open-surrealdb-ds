#!/usr/bin/env bash
# Print `name version` for every `surrealdb-*` crate in a Cargo.lock.
#
# Used by the upstream-drift workflow to report what actually resolved after
# `cargo update`, which is the difference between "CI is red" and "surrealdb-kvs
# 3.3.1 changed the contract". Kept as a script rather than as awk inside a YAML
# heredoc so it can be run and checked by hand.
#
# Usage: locked-versions.sh [path/to/Cargo.lock]
set -euo pipefail

LOCK="${1:-Cargo.lock}"

# Cargo.lock stanza order is `name` then `version`, and the name line may be
# preceded by nothing else inside a [[package]] block, so a two-state machine over
# the two keys is enough. Quotation is stripped from both so the output diffs
# cleanly.
awk '
  /^name = / {
    n = $3
    gsub(/"/, "", n)
    next
  }
  /^version = / {
    if (n != "" && n ~ /^surrealdb/) {
      v = $3
      gsub(/"/, "", v)
      printf "%s %s\n", n, v
    }
    n = ""
  }
' "$LOCK" | sort

# A sanity check that beats a silently empty diff: if the pattern ever stops
# matching Cargo.lock's format, this workflow would report "no version change"
# forever and never notice.
count=$(awk '
  /^name = / { n = $3; gsub(/"/, "", n); next }
  /^version = / { if (n ~ /^surrealdb/) c++; n = "" }
  END { print c + 0 }
' "$LOCK")
if [ "$count" -eq 0 ]; then
  echo "locked-versions.sh: found no surrealdb-* packages in $LOCK" >&2
  exit 1
fi
