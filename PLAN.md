# Plan

Phased roadmap. Each phase has a deliverable, a verification command, and an
explicit exit criterion. A phase is not done until its exit criterion passes.

The ordering is deliberate: **de-risk the seam before building anything on it.**
Phase 0 exists to retire the largest unknown (does the extension point actually
work from an external crate?) in days rather than months.

---

## Phase 0 — Prove the seam

**Goal:** a third-party crate registers a storage backend and passes the upstream
conformance suite.

**Why first.** Everything after this depends on it. If `BackendProvider` or
`Transactable` turns out to be sealed, mis-typed, or unusable from outside the
upstream workspace, the entire architecture changes and we want to know before
writing consensus code.

### Tasks

- [x] `crates/surrealdb-ds` compiles against `surrealdb-kvs` / `surrealdb-kvs-any` 3.3.0
- [x] Implement `BackendProvider` claiming schemes `ds` and `ds+mem`
- [x] Implement `TransactionBuilder` returning an in-memory `Transactable`
- [x] Register via `Backends::register`, construct via
      `Backends::new_transaction_builder` — **verified at runtime**, coexisting
      with `rocksdb:` and `memory`
- [x] `crates/surrealdb-ds-server` builds and runs
- [x] Vendor `surrealdb-kvs-test`, register a `TestBackend`, run the suite — done
      2026-10-02; see ADR-0005 and ADR-0006
- [x] Serve HTTP: `/health`, `/ready`, `/version`, then SurrealQL over `/rpc` —
      done 2026-10-02 via a composer (R-0036), verified end to end by
      `make smoke`, which starts the server and proves a CREATE committed and a
      SELECT read it back

### Exit criterion

```
make check    # all crates type-check        -- PASSING
make test     # upstream conformance suite passes against our engine   -- PASSING
```

**Status: all 8 tasks done, and all three exit-criterion commands are green.**

```
make check      # all crates type-check
make test       # includes the upstream conformance suite
make smoke      # serves SurrealQL on our engine and proves a round trip
```

The engine registers as a SurrealDB backend, passes all 77 runnable tests of
upstream's own backend contract suite, and serves the real SurrealQL front end on
top of it. Twelve conformance tests are reported *ignored* by the suite's own
backend-name filters; what those skips are, and what we test ourselves because of
them, is written out in PROGRESS.md rather than left to whoever reads the output.

Phase 1 is where the remaining risk lives: nothing here is durable, and L2 — byte
compatibility with a real SurrealDB node, which is this project's definition of
1:1 — is still unproven.

---

## Phase 1 — L2 single node

**Goal:** byte-compatible single-node datastore, so our engine and a real
SurrealDB read and write the same data.

### Why byte compatibility matters

It is the strongest available evidence that our understanding of the system is
correct, and it is what makes L2 a meaningful definition of 1:1. The
`key`/`value` encoding contract is a published interface in `surrealdb-kvs`, so
this is legitimate to implement against — subject to ADR-0002.

### Tasks

- [ ] Reconstruct the key encoding contract from the published interface
- [ ] Local durable backend (RocksDB as a dependency of ours, not theirs)
- [ ] Golden-file tests: dataset written by upstream, read by us, and back
- [x] Read-snapshot isolation (`kvs-test::snapshot`) — per-key versions and commit
      stamps, done 2026-10-02
- [ ] Versioned (time-travel) reads (`kvs-test::versioned`) — currently *refused*
      with `UnsupportedVersionedQueries`, which is what the suite requires of every
      backend registered without versioning, us included
- [x] Version GC — live-snapshot pins, horizon-based collection, `history`
      retired; done 2026-10-02. **Known limit carried forward:** a transaction that
      is never dropped pins the horizon for ever, so a leaked `Box<dyn Transactable>`
      stops collection. A query timeout is the fix and does not exist yet.
- [ ] Compaction (`kvs-test::builder_surface::compact_supported`) — still declined

### Exit criterion

Round-trip a non-trivial dataset — graphs, indexes, vectors, permissions —
through upstream → us → upstream with byte equality.

---

## Phase 2 — Transaction semantics conformance

**Goal:** identical transaction behaviour, because this is where silent
divergence is most expensive.

**Highest-risk area in the entire project.** Get conflict semantics wrong and
every read-modify-write in SurrealQL changes meaning — no error, just wrong
data. Three requirements, all black-box testable:

1. Return a **retryable** conflict error on write conflict. The engine must
   distinguish retryable from definite failure, or SDK retry helpers misbehave.
2. `SELECT … FOR UPDATE` must register read records for commit-time conflict
   detection.
3. An indeterminate commit must surface as a distinct outcome that tells the
   client to **read back rather than replay**, because the write may have landed.

### Tasks

- [x] Re-read the `Transactable` contract at v3.3.0 and enumerate every method
- [x] Conflict semantics: read-set validation at commit, retryable refusal, and
      blind writes serialising in stamp order rather than aborting (R-0034) — done
      2026-10-02
- [x] Savepoint semantics (`new` / `release_last` / `rollback_to`) — undo logs with
      nesting and release-merges, done 2026-10-02
- [x] Cursor streaming: keys, values, batch, multi-get — done 2026-10-02
- [ ] Locked reads (`getu`). Refused today, and that refusal is *why* the suite
      reports its four `getu_*` conflict tests as ignored. Plain reads are validated
      at commit, which is stronger than a row lock, but it is not the same path
- [ ] Differential harness against an upstream single-node server, free and unlimited
- [ ] Fuzz: random op sequences compared against upstream, output diffed
- [ ] Live queries end to end. `safe_timestamp` is half of it; the router also needs
      a broker that relays notifications off-node, and
      `TransactionBuilderFactory::live_query_broker` is the hook (R-0037)

### Exit criterion

Zero divergence in the differential harness over a large random op corpus,
including the three requirements above.

---

## Phase 3 — Consensus and replication

**Goal:** a node joins, replicates, and survives the loss of a peer.

### Design constraints already known from public information

- **Leaderless quorum commit.** Per-AZ write nodes; a transaction commits when
  a quorum acknowledges. Every write node can coordinate — no leader bottleneck.
- **Odd node counts (3/5/7).** An even count survives no more failures than N−1,
  and the fast commit path requires all nodes to agree, so one slow node forces
  the slow path.
- **Epoch fencing on membership change.** A fenced node must rejoin the new
  membership or fail retryably, never starve.
- **Recovery cost must be bounded by the log delta, never by dataset size.** The
  upstream 3.3.0 release notes describe fixing a recovery drain that was
  O(all transactions ever committed); any implementation that re-pages a peer's
  entire history has the same defect.
- **Cold-start convergence.** Upstream improved their worst case from 86.9s to
  13.7s in one release. Ours should be measured and reported, not assumed.
- **`safe_timestamp` is not optional.** A distributed backend whose commit log is
  non-linear must report a genuine *closed* watermark, or the live-query router
  silently misses notifications. This is documented in the upstream trait
  comments and it is a correctness property, not an optimisation.

### Tasks

- [ ] Write-node per zone, quorum commit, two-phase finalize with prepare retries
- [ ] Anti-entropy and catch-up
- [ ] Membership change with epoch fencing
- [ ] Bounded recovery drain (journal-based delta request, full-stream fallback)
- [ ] Failure injection harness: kill -9, partition, clock skew, slow node
- [ ] Telemetry following the `surrealdb.ds.*` naming scheme

### Exit criterion

No acknowledged write is ever lost; no divergent outcome survives reconciliation;
kill -9 of one of three nodes loses no committed transaction and serves reads
throughout.

---

## Phase 4 — Durability tier and elasticity

**Goal:** object-storage durability, scale-to-zero, node lifecycle.

### Tasks

- [ ] Object-storage durable tier (S3-compatible, GCS, Azure)
- [ ] Restore-from-log-delta on cold start
- [ ] Node registry and peer discovery. `TransactionBuilderFactory::http_endpoint()`
      is where a node publishes the endpoint it wants recorded on its `Node` catalog
      row, and `datastore_node_id()` is what makes live-query ownership routable
      (R-0037). Resolution of a *peer's* endpoint is a separate thing —
      `dbs::NodeEndpointResolver`, handed to the broker after the datastore is
      built (R-0038)
- [ ] Distributed live queries — `TransactionBuilderFactory::live_query_broker()`
      rather than the default local broker (R-0037)
- [ ] Rolling upgrade with no downtime

### Exit criterion

A node restored from object storage replays only its log delta; scale to zero
and back does not lose a committed write.

---

## Cross-cutting

### Test strategy, in order of value

1. **Upstream conformance suite** (`surrealdb-kvs-test`) — the contract.
2. **Differential harness** against a real upstream server — parity.
3. **Fault injection** — the properties that only appear under failure.
4. **Property tests** — invariants over random histories.
5. **Unit tests** — last, and least, at this stage.

The valuable artifact is "passes the same observable contract", not internal
code quality. Optimise the test suite accordingly.

### Observability from day one

Adopt the `surrealdb.ds.*` metric naming scheme as our own telemetry contract.
It is a ready-made schema covering consensus, networking, view changes, recovery
and GC, and matching it makes our engine legible to anyone who has operated the
real thing.

### Oracle budget

| Oracle | Cost | Covers |
| --- | --- | --- |
| Upstream single-node server | free | Whole front end, transaction semantics |
| Upstream conformance suite | free | KV backend contract |
| Cloud Scale, 3 nodes | ~$420/mo | Distributed *client-visible* semantics |
| Enterprise self-hosted | contact sales | The mesh protocol — which we do **not** target |

Note the gap: no oracle can observe the inter-node wire protocol. That is why L3
is out of scope and L2 is the target.

### Explicitly out of scope

- L3 wire identity with the real mesh (unobservable — see README).
- Any circumvention of SurrealDB's licence-key enforcement.
- Forks or redistributions of SurrealDB code under terms other than BUSL-1.1.
- Parity with SurrealDS features that are documented as unreleased.