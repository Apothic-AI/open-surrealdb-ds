# open-surrealdb-ds

A clean-room reimplementation of the distributed storage engine behind
SurrealDB — the tier that makes SurrealDB horizontally scalable and highly
available, and which SurrealDB Ltd. ships only as a closed, licence-gated
binary.

This project is **not a SurrealDB fork**. The engine is an independent
implementation, written from public documentation, release notes, published
interfaces and black-box observation, that targets the same *observable
behaviour*. One exception, deliberate and recorded: upstream's backend contract
suite is vendored under `vendor/` and linked by our test target — see
[Clean-room discipline](#clean-room-discipline) and ADR-0005.

> **Status: Phase 0 — the seam works, and the engine passes upstream's own
> conformance suite.** A third-party crate registers as a SurrealDB storage
> backend, constructs through its own URL scheme alongside upstream's own
> backends, and passes all 77 runnable tests of SurrealDB's shared backend
> contract suite. Storage is still in-memory and single-node; nothing here is
> durable or replicated. See [PROGRESS.md](PROGRESS.md) for exactly what is
> verified — including what the suite declines to check — and
> [PLAN.md](PLAN.md) for the roadmap.

---

## Why this exists

SurrealDB's query engine — SurrealQL, the planner, graph/vector/document
indexes, permissions, live queries — is excellent, and from v3.3.0 it is split
into cleanly separated crates on crates.io. Its *storage tier*, however, is not
available:

| Tier | Availability |
| --- | --- |
| Single node (`rocksdb:`, `surrealkv:`, `memory`, `tikv:`, IndexedDB) | Published as crates, BUSL-1.1 — not open source |
| Distributed storage ("SurrealDS") | Closed binary, Enterprise licence, contact-sales pricing |

Note the row labels: SurrealDB is [BUSL-1.1, not open source](https://surrealdb.com/legal),
and "available" above means *shipped in every build*, not *free to use as a
database service*. See [Legal](#legal).

SurrealDB ships a documented plug-in point for exactly this — a
`BackendProvider` trait in `surrealdb-kvs-any` whose own doc comments name "the
enterprise distributed store" as the intended external implementor. This project
takes that seam seriously and reimplements what sits behind it.

The intended result is a horizontally scalable, quorum-consistent,
object-storage-backed key-value engine, and a SurrealDB-compatible binary serving
the real SurrealQL front end on top of it. Neither exists yet: the engine is
in-memory and single-node, and the binary constructs a datastore and exits. See
the status box above and [PLAN.md](PLAN.md) for the order they get built in.

---

## How it relates to SurrealDB

```
        ┌──────────────────────────────────────────────┐
        │  SurrealQL · planner · indexes · permissions  │   from SurrealDB's published
        │  graph · vectors · live queries               │   front-end crates (BUSL-1.1)
        └──────────────────────────────────────────────┘
                             │  kvs / kvs-any
                             ▼
        ┌──────────────────────────────────────────────┐
        │  TransactionBuilder + Transactable           │   ← THIS PROJECT
        │  consensus · replication · recovery           │
        │  object-storage durability tier               │
        └──────────────────────────────────────────────┘
```

We reimplement the bottom box. Under ADR-0002 the top box is **reused**, linked
from crates.io — that decision is settled, and it is why this repository is
BUSL-1.1 for everything that links SurrealDB code and Apache-2.0 only for what is
authored here.

---

## Fidelity: what "1:1" means here

Three levels are possible. We target **L2**.

| Level | Meaning | Reachable? |
| --- | --- | --- |
| **L1** | Behavioural equivalence over the client surface: SurrealQL results, error shapes, transaction outcomes, `/metrics` names, `/health` + `/ready` | Yes — fully black-box verifiable |
| **L2** | Datastore byte compatibility: identical key encoding, so our node and a real SurrealDB node read and write the same data | Yes — and now **evidenced** by `make golden`: 55 keys / 1526 value bytes round-trip upstream → us → upstream byte for byte |
| **L3** | Wire identity with the real inter-node mesh: QUIC framing, message-class partitioning, consensus wire messages | **No** — encrypted QUIC between nodes cannot be observed without a licence |

**L2 is the definition of 1:1 for this project.** L3 is not reachable clean-room
and we do not attempt to guess at it.

**What the L2 evidence covers, precisely.** Byte fidelity *at the KV boundary*:
`Transactable` is byte-level, so our tier stores and returns the bytes it is
handed, and the harness proves that against bytes upstream's own engine wrote.
Covered: graph edges, a unique index, an HNSW vector index with its serialised
vectors, record documents, definitions, tombstones, and version lists across nine
steps including a reader pinned at an older version.

Not covered, and not to be read as covered: **on-disk format compatibility**
(our tier is still in-memory), **completeness of the keyspace** (only the key
classes the test workload reaches), and **durability of any kind**. `make golden`
is falsifiable — a single flipped bit in our write path fails it — but a green run
is evidence about bytes, not a claim of equivalence. See `DECISIONS.md` ADR-0007.

---

## Repository layout

```
open-surrealdb-ds/
├── README.md                  this file
├── LICENSE                    Apache-2.0 (this project's own code)
├── NOTICE                     licensing boundary vs. BUSL-1.1 upstream
├── PLAN.md                    phased roadmap and acceptance criteria
├── PROGRESS.md                dated progress log — what works, what doesn't
├── DECISIONS.md               architecture decision records
├── PROVENANCE.md              clean-room source log (the important artifact)
├── Makefile                   bootstrap / check / test / audit targets
├── scripts/
│   ├── bootstrap.sh           fetch + pin the upstream reference tree
│   └── audit-upstream.sh      re-check published crates + licences
├── crates/
│   ├── surrealdb-ds/          the engine: consensus, replication, recovery
│   └── surrealdb-ds-server/   binary that registers the engine and serves it
├── vendor/
│   └── surrealdb-kvs-test/    upstream's backend contract suite (ADR-0005)
└── docs/
    ├── architecture.md        upstream v3.3.0 crate map + the extension seams
    ├── upstream-crates.md     generated by `make audit-upstream`
    ├── research/              research notes (SurrealDB self-hosting, SurrealDS)
    └── spec/                  behavioural specification, written per requirement
```

`upstream/` and `target/` are gitignored and neither is ever published; the
vendored crate under `vendor/` is what the build reads.

---

## Quick start

```bash
make bootstrap      # fetch + pin the upstream reference tree (v3.3.0)
make check          # type-check every crate
make test           # run the test suite, including the conformance suite
make conformance    # run only the upstream KV backend conformance suite
make golden         # round-trip a dataset through upstream -> us -> upstream, byte for byte
make construct      # construct the engine from a path and exit, without serving
make run            # serve SurrealQL on ds+mem:// over HTTP (blocks)
make smoke          # start the server and exercise it end to end
make audit-upstream # re-check published SurrealDB crates and licences,
                    #   regenerating docs/upstream-crates.md
```

`make bootstrap` is only needed to re-read upstream's interfaces; the build does
not depend on it, because the one crate we need from the tree is already
vendored. A fresh clone builds and passes the suite with no network step beyond
fetching crates.

### Continuous integration

Two workflows, because there are two different questions:

| Workflow | Trigger | Question it answers |
| --- | --- | --- |
| `ci` | every push and PR | Does this commit still pass, reproducibly? |
| `upstream-drift` | nightly | Does upstream still let us pass? |

`ci` runs with `--locked`, so a green result means "against the `Cargo.lock` in
this commit". It has two jobs: a fast one that runs only the conformance suite
(it needs four small crates, not the whole of SurrealDB) and a slower one that
type-checks, tests and lints the whole workspace — which is what proves ADR-0003,
that our own binary carries the engine rather than a patched upstream one. That
slower job also names `make golden` as its own step: those tests already run
under `make test --workspace`, but their output is captured, and the project's
central claim deserves a legible line in the run list rather than hiding inside a
test count.

`upstream-drift` is the one that earns its keep. `surrealdb-kvs` states that it
"does not adhere to SemVer and its API is free to change and break code even
between patch versions", and ADR-0001 exists because we were misled by a branch
once already. So this workflow deliberately resolves against whatever is newest
on crates.io, re-runs the suite, diffs the published crate inventory, and **files
an issue** if anything moved — rather than leaving a red tick for someone to
notice weeks later. It is why `make audit-upstream` writes a dated artifact rather
than only printing.

Both use no third-party actions, and the toolchain is pinned to the version the
suite is verified against so that a rustc bump cannot be mistaken for a contract
change.

Verified at the time of writing:

```
$ make test
test surrealds::lifecycle::closed_after_commit                       ... ok
test surrealds::multi::multiwriter_same_keys_allow                  ... ok
test surrealds::raw::put                                            ... ok
test surrealds::savepoint::rollback_reverts_writes                  ... ok
...
test result: ok. 77 passed; 0 failed; 12 ignored

$ make construct
INFO surrealdb_ds_server: engine ready name="surrealds"
ok: constructed backend surrealds for ds+mem://

$ make smoke
  ok   /health -> 200
  ok   /ready -> 200
  ok   /version -> surrealdb-3.3.0
  ok   CREATE over /rpc
  ok   SELECT over /rpc
  ok   the created document is readable back with its value
  ok   an unknown database is refused, not silently accepted
smoke: all checks passed against ds+mem:// over HTTP
```

It also serves: `surrealdb-ds-server start` is a SurrealDB server whose datastore
can be our engine, reached through the same `BackendProvider` seam. Because
`surrealdb_server::init` boots the upstream CLI, the command surface is
upstream's — `start`, `version`, `config` — and only `construct-only` is ours.

The same binary still constructs `rocksdb:`, `memory` and `ds://node1` — our
provider coexists with upstream's rather than replacing it — and rejects an
unknown scheme with a clean error.

The 12 ignored tests are the suite's own per-backend filters, and they are the
interesting part of the result: the suite's vocabulary already names `surrealds`,
so registering under that name puts us behind the assertions reserved for the
engine we are reimplementing — and behind the skips that come with it. What they
are, and what we test ourselves because of them, is in
[PROGRESS.md](PROGRESS.md#what-the-suite-does-not-check-for-us).

---

## Clean-room discipline

This is the part that matters most, and the easiest to get wrong by accident.

1. **Sources of truth are documents, published interfaces and observation — not
   upstream code.** Every requirement in `docs/spec/` carries a provenance record:
   the source class, the citation, the access date. `PROVENANCE.md` is the running
   log, and it records two of our own mistakes as carefully as it records upstream's
   facts.
2. **The upstream tree is a reference, not a build input.** `upstream/` is
   gitignored and never compiled; we read it the way we'd read an API doc. Two
   things are *not* references, because we compile against them, and both bind us
   to BUSL-1.1: the published interface crates (`surrealdb-kvs`,
   `surrealdb-kvs-any`, …) per ADR-0002, and the vendored conformance suite per
   ADR-0005.
3. **The conformance suite is upstream's, and that is deliberate.**
   `surrealdb-kvs-test` is not on crates.io (`publish = false`), so it is vendored
   under `vendor/` — upstream's source, unmodified, BUSL-1.1 — and our test target
   is BUSL-1.1 with it. Passing a contract suite we did not write is the
   strongest available evidence that the engine behaves like a real one; passing
   tests we adapted to our engine would be worth nothing. See ADR-0005.
   Its *skips* are part of the contract too, so what it will not check for us is
   recorded and covered by our own tests. See ADR-0006.
4. **Observations are recorded before they are implemented.** No requirement
   enters `docs/spec/` without a provenance entry.

---

## Legal

This project is licensed **Apache-2.0** for code authored here — which is the
crates that import nothing from SurrealDB, plus the documentation.

SurrealDB is licensed **BUSL-1.1**, not Apache-2.0 (its own `CARGO.md` badge is
inaccurate), so every crate we link is BUSL-1.1. In practice that is
`crates/surrealdb-ds`, `crates/surrealdb-ds-server`, `vendor/surrealdb-kvs-test`,
and their test targets: all three declare `license = "BUSL-1.1"`.

There are three licensing paths in this repository, and they are worth
distinguishing:

| Path | Terms | Where |
| --- | --- | --- |
| Authored here | Apache-2.0 | `PROVENANCE.md`, `docs/`, `scripts/`, `Makefile` |
| Links a SurrealDB crate | BUSL-1.1 (derivative work) | `crates/*` |
| Vendored upstream source | BUSL-1.1, unmodified | `vendor/surrealdb-kvs-test/` |

BUSL-1.1's Additional Use Grant permits use but **not as a "Database Service"**,
and its Change Date is not a planning assumption — it has moved twice, each time
later, and past the nominal four-year mark. Treat the encumbrance as open-ended.
`NOTICE` states the boundary in full; `DECISIONS.md` ADR-0002 records the
reasoning and the rejected alternative.

Nothing in this repository circumvents, and nothing here should be used to
circumvent, SurrealDB's licence-key enforcement.

---

## References

- SurrealDB 3.3.0 release notes — <https://surrealdb.com/releases/3.3>
- Multi-node deployment models — <https://surrealdb.com/docs/running/multi-node>
- Deployment models — <https://surrealdb.com/docs/manage/self-hosted/deployment-models>
- Enterprise Edition — <https://surrealdb.com/docs/manage/enterprise/product/database-enterprise>
- SurrealDB legal terms — <https://surrealdb.com/legal>