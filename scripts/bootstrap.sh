#!/usr/bin/env bash
# Fetch and pin the upstream reference tree.
#
# We pin tag v3.3.0 (commit 238bfeb, 2026-09-24) and NOT the default branch:
# origin/main is ~20 days older than the release and has a materially different
# layout. See ../DECISIONS.md ADR-0001 and ADR-0004.
set -euo pipefail

UPSTREAM_URL="https://github.com/surrealdb/surrealdb.git"
UPSTREAM_REF="v3.3.0"
DEST="$(cd "$(dirname "$0")/.." && pwd)/upstream/surrealdb"

mkdir -p "$(dirname "$DEST")"

if [ -d "$DEST/.git" ]; then
  echo "==> upstream already present at $DEST"
  git -C "$DEST" describe --tags --always
  exit 0
fi

echo "==> cloning $UPSTREAM_URL at $UPSTREAM_REF (blobless, shallow)"
git clone --filter=blob:none --depth 1 --branch "$UPSTREAM_REF" "$UPSTREAM_URL" "$DEST"

echo "==> pinned: $(git -C "$DEST" rev-parse --short HEAD) ($(git -C "$DEST" log -1 --format=%cs))"

cat <<'NOTE'

The upstream tree is a REFERENCE, not a build input.

  * It is used to read published interfaces and documentation.
  * It is never compiled into our crates, and no code from it is copied or
    translated. See ../PROVENANCE.md.

To confirm our layout assumptions still hold, run:

    make audit-upstream

which re-checks which SurrealDB crates exist on crates.io and at what licence.
NOTE
