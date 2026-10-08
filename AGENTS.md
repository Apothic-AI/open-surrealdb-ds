# AGENTS.md

Working notes for coding agents on this repository. Read this before changing
anything.

## What this is

A clean-room reimplementation of SurrealDB's distributed storage engine (the tier
SurrealDB markets as "SurrealDS"). It links SurrealDB's published crates for the
front end and the KV contract, and implements the storage engine itself.

Phase 0 is closed. Phase 1 is byte compatibility, and is **measured rather than
assumed**: L2 round-trip, on-disk interop in both directions, and the durability
properties the local store actually provides. Storage is **still in-memory and
single-node** — nothing is durable or replicated yet, and the durable tier is the
work in progress.

## Where to start

Read, in order: `README.md`, `PROGRESS.md` (newest first — **verified vs
intended**), `DECISIONS.md`, `PLAN.md`, `PROVENANCE.md`, `NOTICE`,
`docs/architecture.md`, `docs/remote-builds.md`.

**The next task is ADR-0012 step 3**, listed in `PLAN.md` Phase 1 with its state:
factor the transaction-semantic layer over a small storage interface, keep the
in-memory store and RocksDB as two implementations of it, and run the same
generated operation traces and conflict schedules against both.

Two results from step 2 constrain step 3 and are easy to get wrong, so they are
repeated here rather than left to whoever needs them:

- **Durability is a parameter, not a property of the store.** Nothing about it
  holds on the defaults. Upstream gets per-commit durability by **grouping**
  commits behind one `flush_wal(true)` — it explicitly never fsyncs per
  transaction, despite a log line that says otherwise (ADR-0013).
- **Do not inherit RocksDB's transaction path.** It rejects overlapping blind
  writes, with *and* without `set_snapshot(true)`, which the `surrealds` contract
  requires to all commit. Take the plain write path and implement commit identity
  ourselves.

Load the `shared-flyio-build-server` skill before using the build server; it is
the operational reference and carries the traps.

## The five commands, and what each proves

```bash
make test        # 19 + 77 conformance (12 ignored) + 6 + 2 + 16 + 3 + 9 + 1, 0 failed
make golden      # L2: 55 keys / 1526 value bytes, upstream -> us -> upstream
make interop     # on-disk, both directions, byte for byte
make durability  # SIGKILL a writer; which durability properties hold, with which knobs
make smoke       # HTTP round trip against ds+mem:// (needs a local binary; see below)
```

`make test`, `make golden` and `make interop` all run under `make test --workspace`
as well; `make interop` and `make durability` exist so the byte and durability
claims have **names** in the output rather than hiding inside a test count. Read
each one's limits in `PROGRESS.md` — a green `make interop` says nothing about
durability, and a green `make durability` measures **recovery, not durability**,
because `SIGKILL` preserves the page cache.

`make smoke` is the one command with no zero-artifact form: it runs a server, so
it needs the binary locally via `--copy-back`. It is also the slowest.

## Build on the shared Fly server, not locally

**The local disk is effectively full** — `/home` has been observed at 100% with
4.2G free. This repo's own `target/debug` was ~32G and has been deleted; the two
largest consumers on the box are now `apothic-monorepo` (~16G) and
`apothic-monorepo-workspace` (~14G), neither of which is ours to clean. So local
building is not merely slow here, it is unavailable, and it will stay that way
until someone reclaims space outside this repository.

```bash
export PATH="$HOME/.cargo/bin:$PATH"

# Every make target takes a CARGO override, so the documented commands work
# verbatim once CARGO points at the remote:
CARGO='cargo remote-3000 -r fly -d 1.95.0 --'

make test CARGO="$CARGO"        # 12 suites, 0 failed
make golden CARGO="$CARGO"
make interop CARGO="$CARGO"
make durability CARGO="$CARGO"
make clippy CARGO="$CARGO"      # not a target; see below
make remote CMD='clippy --workspace --all-targets -- --deny warnings'

make remote-stop                 # stop the box so it stops billing
```

Nothing comes back unless you pass `--copy-back`, so `test`, `clippy` and `check`
return only output. For the server binary that `make smoke` needs:

```bash
cargo remote-3000 -r fly -d 1.95.0 -- build -c=debug/surrealdb-ds-server -p surrealdb-ds-server
```

**Two things are not optional.**

**`-d 1.95.0`** — cargo-remote defaults the toolchain to `stable` and runs
`rustup default` on every build, so omitting it silently installs and switches
compiler versions.

**`--` before the cargo arguments** — cargo-remote parses its own flags *after*
the cargo subcommand too, and `-p` is its `--ssh-port`. So
`... test -p surrealdb-ds-server` dies with `invalid value
'surrealdb-ds-server' for '--ssh-port <PORT>'`. The `--` terminator is what keeps
cargo's `-p` away from cargo-remote's.

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

All of these were green at the last commit, run on the build server:

```bash
make check          # clean, zero warnings
make test           # 19 + 77 conformance (12 ignored) + 6 + 2 + 16 + 3 + 9 + 1, 0 failed
make golden         # L2 round trip: upstream -> us -> upstream, byte for byte
make interop        # both directions of on-disk interop with a real upstream store
make durability     # SIGKILL a writer; which durability properties hold, with which knobs
make smoke          # HTTP round trip against ds+mem:// (needs a local binary)
cargo clippy --workspace --all-targets -- --deny warnings   # clean
```

`make golden`, `make interop` and `make durability` need a **running** build
server. `make smoke` is the slow one and is the only command needing a local
binary. A green conformance run does **not** mean full
coverage — 12 tests are reported ignored by the suite's own backend-name filters,
and PROGRESS.md lists exactly which and what we test instead.

## Ground rules

- **Commit only when asked.** Conventional commits; bodies state what was
  actually verified, including what a green run does *not* cover.
- **An entry claiming more than was checked is worse than no entry.** When a
  delegated or self-run result surprises you — including by contradicting a brief
  you wrote — verify it yourself before recording it. That habit found every
  substantive error in this project's history so far.
- **Never commit `target/` or `upstream/`.** Both gitignored.
- **`crates/surrealdb-ds/src/storage.rs` must never import `surrealdb_kvs`.**
  It is deliberately dependency-free so it survives a move to an independent
  implementation (ADR-0002). Return a local error type and map it in `txn.rs`.
- **Add a provenance record (`R-NNNN`, next free R-0069) for any new
  requirement**, with a source class (`DOC` · `REL` · `OBS` · `PUBAPI` · `SRC`
  — interface shape only — · `BIN`, never used) and a precise citation.
- **An ADR for architecture decisions.** Next free is ADR-0014. Do not invent
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