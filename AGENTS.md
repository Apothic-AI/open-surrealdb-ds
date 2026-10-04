# AGENTS.md

Working notes for coding agents on this repository. Read this before changing
anything.

## What this is

A clean-room reimplementation of SurrealDB's distributed storage engine (the tier
SurrealDB markets as "SurrealDS"). It links SurrealDB's published crates for the
front end and the KV contract, and implements the storage engine itself.

Phase 0 is closed: the engine registers as a KV backend and passes upstream's own
contract suite. Phase 1 is byte compatibility.

Read, in order: `README.md`, `PROGRESS.md` (newest first — **verified vs
intended**), `DECISIONS.md`, `PLAN.md`, `PROVENANCE.md`, `NOTICE`,
`docs/architecture.md`, and `docs/remote-builds.md`.

Load the `shared-flyio-build-server` skill before using the build server; it is
the operational reference and carries the traps.

## Build on the shared Fly server, not locally

**The local disk is full.** `target/debug` alone is ~32G against ~18G free, so
local building is impossible rather than merely slow.

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo remote-3000 -r fly -d 1.95.0 check  --workspace --all-targets
cargo remote-3000 -r fly -d 1.95.0 clippy --workspace --all-targets -- --deny warnings
cargo remote-3000 -r fly -d 1.95.0 test   --workspace
```

Nothing comes back unless you pass `--copy-back`, so these return only output.
For the server binary that `make smoke` needs:

```bash
cargo remote-3000 -r fly -d 1.95.0 build -c=debug/surrealdb-ds-server -p surrealdb-ds-server
```

**`-d 1.95.0` is not optional** — cargo-remote defaults the toolchain to
`stable` and runs `rustup default` on every build, so omitting it silently
installs and switches compiler versions.

The box is stopped when idle, so start it first (see the skill). Its root
filesystem is ephemeral, so packages must be reinstalled after every restart.

Local `cargo` still works and is the fallback when the machine is unavailable.
`make check` and `make test` are what CI runs. If you must build locally, do not
run `cargo clean` casually: it is 32G you may need back.

## Verify before you claim

`PROGRESS.md` exists to separate **verified** from **intended**, and its accuracy
is the project's most valuable property. An entry claiming more than was checked
is worse than no entry. Before writing that something works, run the command and
paste what it printed.

All of these were green at the last commit:

```bash
make check          # clean, zero warnings
make test           # includes the 77-test upstream conformance suite
make golden         # L2 round trip: upstream -> us -> upstream, byte for byte
make smoke          # HTTP round trip against ds+mem://
cargo clippy --workspace --all-targets -- --deny warnings
```

`make smoke` is the slow one. A green conformance run does **not** mean full
coverage — 12 tests are reported ignored by the suite's own backend-name filters,
and PROGRESS.md lists exactly which and what we test instead.

## Ground rules

- **Commit only when asked.** Conventional commits; bodies state what was
  actually verified, including what a green run does *not* cover.
- **Never commit `target/` or `upstream/`.** Both gitignored.
- **`crates/surrealdb-ds/src/storage.rs` must never import `surrealdb_kvs`.**
  It is deliberately dependency-free so it survives a move to an independent
  implementation (ADR-0002). Return a local error type and map it in `txn.rs`.
- **Add a provenance record (`R-NNNN`, next free R-0050) for any new
  requirement**, with a source class (`DOC` · `REL` · `OBS` · `PUBAPI` · `SRC`
  — interface shape only — · `BIN`, never used) and a precise citation.
- **An ADR for architecture decisions.** Next free is ADR-0009. Do not invent
  numbers that are already taken.
- **`docs/spec/` stays empty** unless a record backs it.
- **Do not reimplement what already exists and works** (ADR-0002). Link it.
- **Upstream is pinned to tag `v3.3.0`.** `origin/main` is *older* and on a
  divergent lineage; trusting it has already cost a full re-analysis once
  (ADR-0001).
- **Prefer the published crate source as authority** over any checkout:
  `ls -d ~/.cargo/registry/src/*/surrealdb-<crate>-3.3.0`. A source-tree fact
  is a hypothesis; a published-artifact fact is evidence (ADR-0004).
- **Do not add a dependency without saying why in the report.** The lockfile is
  part of the licensing record.
- **Zero warnings**, including clippy at `--deny warnings`.

## Traps

- `surrealdb_server::init` builds its own tokio runtime and `block_on`s, so
  calling it inside `#[tokio::main]` panics. It also installs the global tracing
  subscriber, and it does an online version check at startup. Prefer
  `surrealdb_server::core`, which reaches `surrealdb_core` and builds a
  `Datastore` against any registry in-process (R-0046).
- `RETURN 1` is evaluated without resolving the database, so it returns OK
  against any database name. Useless as a check that a datastore is real; use a
  real `SELECT`.
- The conformance suite has one contract **per backend name**. We register as
  `surrealds` deliberately (ADR-0006) — the stricter choice. Blind writes to one
  key all commit and serialise in stamp order; that is not first-committer-wins.
- `new_transaction_builder` **is** the store open. `ds+mem://` is fresh per
  call, so writing and reading across two calls compares a full store to an
  empty one (R-0047).
- Version GC stops if a transaction is never dropped. Do not leak a
  `Box<dyn Transactable>`; drop transactions and cursors.
- `make golden-update` rewrites the golden manifest. It fails unless our engine
  already reproduces it, but read the diff before committing it.