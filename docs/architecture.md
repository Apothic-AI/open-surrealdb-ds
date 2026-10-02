# Architecture

How the upstream v3.3.0 tree is organised, where our work goes, and why.

All statements here are pinned to tag `v3.3.0` (commit `238bfeb`, 2026-09-24).
The default branch is *not* the release — see ADR-0001.

---

## The upstream crate map

The KV layer was split out of the monolithic core into purpose-specific crates.
The split is unusually clean, and it is the reason this project is tractable.

```
  surrealdb-kvs ──────────── the contract every backend implements
    │  Transactable            transaction over raw Key/Val bytes
    │  TransactionBuilder      datastore abstraction + metrics
    │  key / value             the encoding contract (L2 lives here)
    │  cursor, err, config, savepoint, timestamp, destroy
    │
    ├── surrealdb-kvs-mem       in-memory (SurrealMX)
    ├── surrealdb-kvs-rocksdb   RocksDB
    ├── surrealdb-kvs-surrealkv SurrealKV
    ├── surrealdb-kvs-tikv      TiKV
    ├── surrealdb-kvs-indxdb    IndexedDB
    │
    ├── surrealdb-kvs-any  ◄─── THE SEAM
    │     Backends               registry: path scheme → provider
    │     BackendProvider        the plug-in trait we implement
    │     ConnectContext         scheme, path, canceller, config
    │
    └── surrealdb-kvs-test  ◄─── the conformance suite (publish = false)
          builder_surface, defaults, edges, lifecycle, multi, raw,
          savepoint, snapshot, timestamp, versioned

  surrealdb-cnf        configuration primitives
  surrealdb-catalog     schema: namespaces, databases, tables, indexes
  surrealdb-datastore   keyspace: which keys exist, durable value shapes
  surrealdb-engine-api  service-provider interface between SDK and engines
  surrealdb-server      HTTP/RPC surface, CLI
  surrealdb-node        node membership (unpublished)
```

The layering statement in `surrealdb-kvs` is worth quoting, because it defines
the boundary we implement against:

> This crate defines how a key and a value are *declared*; it does not declare
> any. Which keys exist is the keyspace, which lives above it.

---

## The seam

```rust
pub trait BackendProvider: Send + Sync {
    fn schemes(&self) -> &[&'static str];
    fn accepts_bare(&self) -> bool { false }
    fn connect<'a>(
        &'a self,
        ctx: ConnectContext<'a>,
    ) -> BoxFut<'a, Result<Box<dyn TransactionBuilder>>>;
}
```

Three methods. The intended use is stated in the upstream docs themselves:

> Providers let external crates plug custom backends into the same
> connection-path dispatch used by the first-party engines.

and the registry carries a test `ExternalProvider` described as mimicking "how an
embedder (e.g. the enterprise distributed store) plugs into the registry".

`ConnectContext` gives us the matched `scheme`, the `path` with the query string
stripped and absolute paths normalised, a `CancellationToken` for graceful
shutdown, and a `ConfigMap` with any URL `?key=value` parameters merged under
`datastore_`-prefixed keys.

Providers are consulted in registration order; first claimant of a scheme wins.

---

## Where our work goes

```
┌─ upstream ───────────────────────────────────────────────┐
│  SurrealQL · planner · index fusion · HNSW/DiskANN         │
│  graph adjacency · permissions · live queries · changefeeds │
│  DEFINE API · WASM extensions · Postgres wire protocol      │
└───────────────────────���───────────────────────────────────┘
                          │  surrealdb-kvs (Transactable)
                          ▼
┌─ THIS PROJECT ───────────────────────────────────────────┐
│  crates/surrealdb-ds                                      │
│    ├── txn/        transaction lifecycle, conflict detect │
│    ├── consensus/  leaderless quorum, epochs, view change │
│    ├── replicate/   catch-up, anti-entropy, bounded drain │
│    ├── storage/     local engine, object-storage tier     │
│    ├── node/        membership, endpoint resolution       │
│    └── observe/     surrealdb.ds.* telemetry              │
└───────────────────────────────────────────────────────────┘
```

We inherit the entire query layer. Our surface area is the transaction contract
plus everything required to make that contract meaningful across nodes.

### Why this is tractable

Everything above the KV contract — the part that is genuinely hard to
replicate, and where SurrealDB's differentiation lives — comes from Apache-2.0-era
published crates and is identical by construction. We are not approximating the
front end; we are using it.

### Why it is not trivial

The KV contract is where all the hard distributed-systems correctness lives, and
it has no safety net: a subtly wrong conflict rule or timestamp watermark
produces silently wrong data rather than an error. Upstream's own 3.3.0 notes
record a lost-write race that "could lose an acknowledged write and let a
`UNIQUE` index hold a duplicate". This is the part to be paranoid about.

---

## The three correctness properties we must not get wrong

**1. Conflict detection must be retryable and distinguishable.**
The engine has to tell "this failed and you may retry" apart from "this
definitely failed". Conflating them makes SDK retry helpers either drop writes
or replay writes that already landed.

**2. Indeterminate commits must be reported as such.**
An outcome that cannot be determined must instruct the client to *read back*
rather than replay, because the write may have committed.

**3. `safe_timestamp` must be a genuine closed watermark.**
This is documented in the upstream trait contract:

> A distributed backend whose commit log is non-linear […] MUST override this to
> return a genuine closed/safe watermark, or the router can miss notifications.

If the highest committed timestamp can float above an unapplied lower one — which
is exactly what a multi-zone quorum log does — then returning `timestamp()` is a
correctness bug, not a missed optimisation. Live queries silently drop changes.

---

## Transaction contract

At the pinned tag the transaction trait is re-exported from `surrealdb-kvs::api`.
The precise method set must be re-read before implementation — the pre-release
tree had 32 methods, and this is the authoritative list to build against.

Known shape from the interface: `kind`, `closed`, `writeable`, `cancel`,
`commit`, point operations (`exists`/`get`/`set`/`put`/`putc`/`del`/`delc`),
range operations (`keys`/`keysr`/`scan`/`scanr`/`count`/`getp`/`getr`/`delp`/
`delr`/`clrp`/`clrr`), batch (`batch_keys`, `batch_keys_vals`), multi-get
(`getm`), cursors (`open_keys_cursor`, `open_vals_cursor`), savepoints
(`new_save_point`, `release_last_save_point`, `rollback_to_save_point`), and
timestamping (`timestamp`, `safe_timestamp`, `timestamp_impl`).

**Do not implement from this summary.** Read `surrealdb-kvs` at the pinned tag.

---

## Telemetry

Adopt the `surrealdb.ds.*` naming scheme for our cluster metrics — roughly thirty
instruments covering network, consensus, view changes, recovery and garbage
collection. Names visible in public sources include:

```
surrealdb_ds_consensus_fast_quorum_timeouts_total
surrealdb_ds_finalize_prepare_retries_total
surrealdb_ds_recovery_outcome_drain*
surrealdb.ds.epoch_fence_drops        { direction }
```

Those names encode the protocol: two-phase finalize with prepare retries,
membership epochs with directional fencing, view changes, and bounded
journal-based recovery. Adopting the scheme makes our engine legible to anyone
who has operated the real one, and gives us a debugging vocabulary for free.

---

## Licensing boundary

Upstream is **BUSL-1.1**, not Apache-2.0. Compiling against
`surrealdb-kvs` / `surrealdb-kvs-any` makes our crates derivative works subject
to BUSL. See NOTICE and ADR-0002.

The design consequence, and the reason the crate boundary is drawn where it is:
algorithmic modules — consensus, replication, recovery — hold no SurrealDB imports
and can move to a fully independent implementation without being rewritten. Only
the boundary types would change.