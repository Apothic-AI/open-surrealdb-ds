#!/usr/bin/env bash
# Re-verify our layout assumptions against PUBLISHED artifacts rather than the
# upstream default branch. See ../DECISIONS.md ADR-0004 — we got this wrong once.
set -euo pipefail

CRATES=(surrealdb-kvs surrealdb-kvs-any surrealdb-kvs-rocksdb surrealdb-kvs-surrealkv \
        surrealdb-kvs-mem surrealdb-kvs-tikv surrealdb-kvs-test \
        surrealdb-engine-api surrealdb-engine-local surrealdb-datastore \
        surrealdb-catalog surrealdb-node surrealdb-cnf surrealdb-server surrealdb)

printf '%-28s %-12s %s\n' CRATE VERSION LICENSE
printf '%-28s %-12s %s\n' '----' '-------' '-------'
for c in "${CRATES[@]}"; do
  out=$(curl -s -H 'User-Agent: open-surrealdb-ds-audit' "https://crates.io/api/v1/crates/$c")
  read -r ver lic <<<"$(printf '%s' "$out" | python3 -c '
import sys, json
try:
    d = json.load(sys.stdin)
    if "crate" in d:
        c = d["crate"]
        ver = c.get("max_stable_version") or c.get("max_version") or "-"
        # The licence lives on the version, not the crate, for BUSL crates
        # (which publish with license = "non-standard" and a license-file).
        lic = "-"
        for v in d.get("versions", []):
            if v.get("num") == ver or v.get("yanked") is False:
                lic = v.get("license") or "-"
                if lic != "-":
                    break
        if lic == "-":
            lic = c.get("license") or "-"
        print(ver, lic)
    else:
        print("absent", "-")
except Exception:
    print("error", "-")
')"
  printf '%-28s %-12s %s\n' "$c" "$ver" "$lic"
done

cat <<'NOTE'

Expectations to re-check when this output changes:

  * `license = non-standard` means BUSL-1.1. If any crate reports Apache-2.0 or
    MIT, ADR-0002's Path A cost drops — re-evaluate.
  * A crate appearing that we don't depend on may mean a layout change.
  * `surrealdb-kvs-test` and `surrealdb-node` are expected to be ABSENT from
    crates.io (publish = false). They must come from the vendored tree.

The upstream default branch is NOT the release. Pin the tag. See ADR-0001.
NOTE
