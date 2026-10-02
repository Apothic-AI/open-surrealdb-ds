# open-surrealdb-ds

A clean-room reimplementation of the distributed storage engine behind
SurrealDB — the tier that makes SurrealDB horizontally scalable and highly
available, and which SurrealDB Ltd. ships only as a closed, licence-gated
binary.

This project is **not a SurrealDB fork**. It is an independent implementation,
written from public documentation and black-box observation, that targets the
same *observable behaviour*.

> **Status: Phase 0 — the seam works.** We have a compiling engine crate that
> registers as a SurrealDB storage backend and constructs through its own URL
> scheme, alongside upstream's own backends. Storage is in-memory and
> single-node; nothing here is durable, concurrent, or production-ready.
> See [PROGRESS.md](PROGRESS.md) for exactly what is verified and
> [PLAN.md](PLAN.md) for the roadmap.

---

## Why this exists

SurrealDB's query engine — SurrealQL, the planner, graph/vector/document
indexes, permissions, live queries — is excellent, and from v3.3.0 it is split
into cleanly separated crates on crates.io. Its *storage tier*, however, is not
available:

| Tier | Availability |
| --- | --- |
| Single node (`rocksdb`, `surrealkv`, `mem`) | Open, in every build |
| Distributed storage ("SurrealDS") | Closed binary, Enterprise licence, contact-sales pricing |

SurrealDB ships a documented plug-in point for exactly this — a
`BackendProvider` trait in `surrealdb-kvs-any` whose own doc comments name "the
enterprise distributed store" as the intended external implementor. This project
takes that seam seriously and reimplements what sits behind it.

The result is a horizontally scalable, quorum-consistent, object-storage-backed
key-value engine, and a SurrealDB-compatible binary that serves the real
SurrealQL front end on top of it.

---

## How it relates to SurrealDB

```
        ┌──────────────────────────────────────────────┐
        │  SurrealQL · planner · indexes · permissions  │   from SurrealDB (Apache-2.0 era
        │  graph · vectors · live queries               │   front-end crates) or ours
        └──────────────────────────────────────────────┘
                             │  kvs / kvs-any
                             ▼
        ┌──────────────────────────────────────────────┐
        │  TransactionBuilder + Transactable           │   ← THIS PROJECT
        │  consensus · replication · recovery           │
        │  object-storage durability tier               │
        └──────────────────────────────────────────────┘
```

We reimplement the bottom box. The top box is either reused (fast path) or
independently implemented (clean path) — see [DECISIONS.md](DECISIONS.md) ADR-0002.

---

## Fidelity: what "1:1" means here

Three levels are possible. We target **L2**.

| Level | Meaning | Reachable? |
| --- | --- | --- |
| **L1** | Behavioural equivalence over the client surface: SurrealQL results, error shapes, transaction outcomes, `/metrics` names, `/health` + `/ready` | Yes — fully black-box verifiable |
| **L2** | Datastore byte compatibility: identical key encoding, so our node and a real SurrealDB node read and write the same data | Yes — the key/value encoding contract is a published interface |
| **L3** | Wire identity with the real inter-node mesh: QUIC framing, message-class partitioning, consensus wire messages | **No** — encrypted QUIC between nodes cannot be observed without a licence |

**L2 is the definition of 1:1 for this project.** L3 is not reachable clean-room
and we do not attempt to guess at it.

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
└── docs/
    ├── architecture.md        upstream v3.3.0 crate map + the extension seam
    ├── research/              research notes (SurrealDB self-hosting, SurrealDS)
    └── spec/                  behavioural specification, written per requirement
```

---

## Quick start

```bash
make bootstrap     # fetch + pin the upstream reference tree (v3.3.0)
make check         # type-check every crate
make test          # run the test suite
make run           # construct our engine from ds+mem:// and exit
make audit-upstream # re-check published SurrealDB crates and their licences
```

Verified at the time of writing:

```
$ make run
INFO surrealdb::core::kvs::ds: Starting kvs store in ds+mem
INFO surrealdb_ds::provider: surrealdb-ds: constructing engine scheme="ds+mem" path=""
INFO surrealdb_ds_server: engine ready: surrealds
ok: constructed backend for ds+mem://
```

The same binary also constructs `rocksdb:`, `memory` and `ds://node1` — our
provider coexists with upstream's rather than replacing it — and rejects an
unknown scheme with a clean error.

---

## Clean-room discipline

This is the part that matters most, and the easiest to get wrong by accident.

1. **Sources of truth are documents and observation, not code.** Every
   requirement in `docs/spec/` carries a provenance record: the public URL, the
   access date, and whether it came from documentation, a release note, or
   black-box experiment. `PROVENANCE.md` is the running log.
2. **The upstream tree is a reference, not a dependency of our logic.** We read
   it the way we'd read an API doc. The one exception is the *interface* we
   compile against (`surrealdb-kvs`, `surrealdb-kvs-any`) — and where we compile
   against it, we are bound by its licence. See ADR-0002.
3. **The conformance suite is upstream's, and that is deliberate.**
   `surrealdb-kvs-test` is a published-but-unlisted shared contract suite for
   exactly this kind of backend. Passing it is the strongest available evidence
   that our engine behaves like a real one. It is BUSL-1.1, so our test target
   is too.
4. **Observations are recorded before they are implemented.** No requirement
   enters `docs/spec/` without a provenance entry.

---

## Legal

This project is licensed **Apache-2.0** for code authored here.

SurrealDB is licensed **BUSL-1.1**, not Apache-2.0, and every crate we may link
is therefore BUSL-1.1. Any crate here that links a SurrealDB crate is a
derivative work and is licensed BUSL-1.1 instead. `NOTICE` states the boundary
in full; `DECISIONS.md` ADR-0002 records the reasoning and the alternatives.

Nothing in this repository circumvents, and nothing here should be used to
circumvent, SurrealDB's licence-key enforcement.

---

## References

- SurrealDB 3.3.0 release notes — <https://surrealdb.com/releases/3.3>
- Multi-node deployment models — <https://surrealdb.com/docs/running/multi-node>
- Deployment models — <https://surrealdb.com/docs/manage/self-hosted/deployment-models>
- Enterprise Edition — <https://surrealdb.com/docs/manage/enterprise/product/database-enterprise>
- SurrealDB legal terms — <https://surrealdb.com/legal>