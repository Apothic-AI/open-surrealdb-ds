# Vendored: `surrealdb-kvs-test`

Upstream's shared behaviour suite for SurrealDB key-value backends, copied here
verbatim so our engine can be held to the same contract the first-party engines
are held to. See ADR-0005 for the decision to do this.

## Provenance

| | |
| --- | --- |
| Upstream path | `surrealdb/kvs-test` in `github.com/surrealdb/surrealdb` |
| Upstream ref | tag `v3.3.0`, commit `238bfeb` (2026-09-24) |
| Upstream licence | BUSL-1.1 — see `LICENSE` in this directory |
| Published on crates.io | **No.** `publish = false`, so a path dependency is the only way in |
| Fetched by | `make bootstrap` → `upstream/surrealdb/` (ADR-0001: the tag, not the default branch) |

`src/` is upstream's, byte for byte. `UPSTREAM-README.md` is upstream's crate
README. `Cargo.toml` is ours: upstream inherits its metadata and lint table from
the workspace root, which does not exist here, so those fields are spelled out
with the values the v3.3.0 workspace supplies. Every other difference between
this and upstream is a difference we had to make, not a change to the tests.

## Re-vendoring

If upstream changes the suite, do not hand-edit `src/`. Re-copy it and record the
new ref:

```bash
make bootstrap
cp -r upstream/surrealdb/surrealdb/kvs-test/src vendor/surrealdb-kvs-test/src
cp upstream/surrealdb/surrealdb/kvs-test/README.md vendor/surrealdb-kvs-test/UPSTREAM-README.md
cp upstream/surrealdb/LICENSE vendor/surrealdb-kvs-test/LICENSE
cargo test -p surrealdb-ds --test kvs
```

Diff the copy against the new ref before trusting a green run: a test that
silently disappeared is a weaker contract than the one we recorded.

## Licensing consequence

This crate is BUSL-1.1, so **our test target is too.** `crates/surrealdb-ds`'s
test target links both this crate and the published `surrealdb-kvs`, and
`crates/surrealdb-ds` already carries `license = "BUSL-1.1"` for that reason
(ADR-0002). The repository `LICENSE` (Apache-2.0) covers nothing in this
directory.

The vendoring is deliberate and narrow: one crate, unmodified, as a build input.
The wider firewall — that `upstream/surrealdb/` is a *reference* and never a
build input — is untouched, because the tree itself is still only read.