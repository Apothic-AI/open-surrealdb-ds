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
└───────────────────────────────────────────────────────────┘
                          │  surrealdb-kvs (Transactable)
                          ▼
┌─ THIS PROJECT ───────────────────────────────────────────┐
│  crates/surrealdb-ds                                      │
│    ├── txn.rs        transactions, conflict detection    │
│    ├── consensus.rs  leaderless quorum, epochs, view change│
│    ├── replicate.rs  catch-up, anti-entropy, bounded drain│
│    ├── storage.rs    versioned local tier, durable tier  │
│    ├── provider.rs   BackendProvider          [tied]      │
│    └── builder.rs    TransactionBuilder      [tied]      │
│                                                          │
│  crates/surrealdb-ds-server: registers the engine, serves │
│    it over HTTP through a surrealdb-server composer      │
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

`Transactable` lives in `surrealdb-kvs::api` and has **36 methods** at the pinned
tag. The pre-release tree had 32, and its shape differed; a summary of the old
one is worse than nothing here, so this is the enumerated, verified list.

**19 with no default** — a backend must implement each:

| Group | Methods |
| --- | --- |
| Lifecycle | `kind` `closed` `writeable` `cancel` `commit` |
| Points | `exists` `get` `set` `put` `putc` `del` `delc` |
| Ranges | `keys` `keysr` `scan` `scanr` |
| Savepoints | `new_save_point` `release_last_save_point` `rollback_to_save_point` |

**17 with a default** — implemented in terms of the ones above:

| Group | Methods | Default's shape |
| --- | --- | --- |
| Locked read | `getu` | refuses with `UnsupportedLockedReads` |
| Cursors | `open_keys_cursor` `open_vals_cursor` | wraps `keys`/`scan`, advancing `range.start` between batches |
| Aliases | `replace` `clr` `clrc` | delegate to `set`/`del`/`delc` |
| Batched reads | `getm` `getr` `count` `delr` `clrr` | loop over `batch_keys`/`batch_keys_vals`; the last three also enforce closed/read-only themselves |
| Batches | `batch_keys` `batch_keys_vals` | page `keys`/`scan`, deriving the continuation from `Key::next()` |
| Clock | `timestamp` `safe_timestamp` `timestamp_impl` | HLC, or an incrementing counter under `test-inc-timestamp` |
| Maintenance | `compact` | refuses with `CompactionNotSupported` |

Two things follow that are easy to get wrong:

- **`Key::next()` appends a zero byte, not `0xff`.** `batch_keys`' default
  derives its continuation range that way, and the suite pins the difference:
  appending `0xff` jumps over every key in between, silently dropping rows at
  batch boundaries. Upstream shipped that bug.
- **The batched defaults enforce `TransactionFinished` and `TransactionReadonly`
  themselves.** Overriding them is only worth it for a backend whose native
  batching is cheaper than the loop — and a hand-written override that forgets
  those checks fails the lifecycle contract while passing everything else.

Our engine overrides 28 of the 36 and leaves the 8 alias/batch defaults
(`replace`, `clr`, `clrc`, `delr`, `clrr`, `batch_keys`, `batch_keys_vals`,
`timestamp_impl`). See `crates/surrealdb-ds/src/txn.rs`.

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

### What we publish today

Six counters, under the group name `surrealdb.ds`, each backed by a counter the
storage tier maintains: `transactions_committed`, `transactions_cancelled`,
`transactions_conflicted`, `keys_written`, `keys_deleted`,
`value_bytes_written`.

The consensus-side instruments above have no honest value yet — quorum size,
prepare retries, view changes, recovery bytes replayed — so they are **absent
rather than zero**, and an undeclared name returns `None` rather than `0`. A
collector can then tell "this backend does not publish it" from "it happens to be
zero", which is the difference between a missing series and a flat one.

---

## The second seam: getting a server in front of the engine

`BackendProvider` gets the engine *constructed*. Serving it is a different seam,
and at v3.3.0 it is:

```rust
pub fn init<C>(composer: C) -> ExitCode
where
    C: TransactionBuilderFactory + RouterFactory + ConfigCheck + ObservabilityProvider;
```

`TransactionBuilderFactory` is where a caller-built `Backends` registry goes.
Upstream's own `CommunityComposer` implementation of it is a three-line
delegation to `Backends::community()` and `Backends::path_valid()`, so ours is the
same with `DsBackend` registered in between, returning
`TransactionBuilderParts::without_router_state(builder)`. Nothing about the CLI's
backend list needs patching (ADR-0003).

The same trait carries the clustered-deployment hooks, which is where the
distributed phases attach rather than the KV crate:

| Hook | Purpose | Phase |
| --- | --- | --- |
| `datastore_node_id()` | stable node identity, so remote writers can route notifications back to the node owning a subscription | 3 |
| `live_query_broker()` | forward notifications to other nodes instead of the default local broker | 4 |
| `http_endpoint()` | the endpoint to record on this node's `Node` catalog row, so peers can find it | 4 |

---

## The conformance suite

`surrealdb-kvs-test` is `publish = false`, so it is vendored — upstream's source,
unmodified, BUSL-1.1 — under `vendor/surrealdb-kvs-test/`. Four dependencies,
nothing else dragged in.

Two facts about it shape how it is used:

1. **It has one contract per backend *name*.** Tests carry `only = [...]` /
   `except = [...]` against the name a consumer registers, and the vocabulary
   already contains `surrealds`. Registering under that name is both the honest
   choice and the stricter one — see ADR-0006 for what it obliges us to.
2. **Its skips are part of the contract.** Registering as `surrealds` means the
   four `getu_*` conflict tests and `write_skew_permitted` are reported *ignored*.
   A green run has to be read next to that list, so the list is written down in
   PROGRESS.md and the serializability it stops checking is asserted in
   `crates/surrealdb-ds/tests/serializable.rs`.

---

## Licensing boundary

Upstream is **BUSL-1.1**, not Apache-2.0. Compiling against
`surrealdb-kvs` / `surrealdb-kvs-any` makes our crates derivative works subject
to BUSL. See NOTICE and ADR-0002.

The design consequence, and the reason the crate boundary is drawn where it is:
algorithmic modules — consensus, replication, recovery — hold no SurrealDB imports
and can move to a fully independent implementation without being rewritten. Only
the boundary types would change.