# Provenance log

The clean-room audit trail. One record per requirement, kept as requirements are
discovered — not reconstructed later, when it stops being credible.

**Rule:** nothing enters `docs/spec/` without a record here.

---

## Source classes

| Class | Code | Meaning | Allowed |
| --- | --- | --- | --- |
| `DOC` | Public documentation | Published prose describing behaviour | yes |
| `REL` | Release notes | Published description of what changed | yes |
| `OBS` | Black-box observation | Behaviour we measured against a running instance | yes |
| `PUBAPI` | Published crate interface | Signatures/types in a crate we compile against | yes — subject to ADR-0002 |
| `SRC` | Upstream source code | Implementation internals | **read for interface shape only; never copied or translated** |
| `BIN` | Binary inspection | Reverse engineering | **no** |

---

## Status of each class in this project

| Class | State |
| --- | --- |
| `DOC` | Used extensively. Primary source. |
| `REL` | Used heavily. The richest public window into SurrealDS internals, because its bug fixes describe real failure modes. |
| `OBS` | Not started. Required before Phase 2 — this is what makes it clean-room. |
| `PUBAPI` | Used. `surrealdb-kvs` / `surrealdb-kvs-any` / `surrealdb-server` / `surrealdb-core` 3.3.0. BUSL-1.1, see ADR-0002. |
| `SRC` | Read for interface shape during archaeology, and — since ADR-0005 — for the **contract assertions** of the vendored conformance suite. **No code copied, translated, or paraphrased into our source.** Findings recorded as requirements here with a citation. |
| `BIN` | **Never used.** Declared off-limits. |

**Firewall.** `upstream/surrealdb/` — a pinned checkout used for reading
interfaces and as a black-box oracle — is kept textually and operationally
separate from the crates we build. It is **never a build input**. The only
SurrealDB code in the build graph is the published crates we deliberately link,
plus one vendored crate (ADR-0005), and that is a recorded decision, not an
accident.

**One correction worth recording.** Our first architecture pass read the upstream
default branch and drew twelve conclusions from it. Checking crates.io showed
that branch predates the v3.3.0 release by twenty days and has a different
layout, so most of those conclusions were wrong — including the shape of the
extension seam and whether the storage contract was even public. Ten of twelve
API assumptions in our first implementation were also wrong. Treat every
branch-derived claim as a hypothesis until a published artifact confirms it
(ADR-0001, ADR-0004).

**A second correction, smaller but of the same kind.** R-0030 recorded that
v3.3.0's `TransactionBuilder` has no `extension()` hook. It has one, with a
default returning `None` — the hook exists and simply was not overridden. The
mistake was reading "our implementation does not implement it" as "the trait has
no such method", which a defaulted method makes indistinguishable from the
outside. R-0031 supersedes it. The generalisation is the ADR-0004 one: a fact
about an interface is established by reading the interface, not by noticing
which parts of it you happened to use.

---

## Requirement records

Format: each requirement gets an ID, a statement, a source class, a citation,
and a date. Add new records as work proceeds; never edit a citation in place.

### Inventory

| ID | Requirement | Source | Citation | Date | Status |
| --- | --- | --- | --- | --- | --- |
| R-0001 | Community Edition supports only single-node storage: `rocksdb`, `surrealkv` (beta), `mem`, `indxdb` | DOC | surrealdb.com/docs/manage/self-hosted/deployment-models | 2026-09-30 | recorded |
| R-0002 | `tikv://` remains compiled in but is documented as local multi-node experimentation only | DOC | surrealdb.com/docs/reference/cli/surrealdb-cli/commands/start | 2026-09-30 | recorded |
| R-0003 | Horizontal scaling requires a distributed storage engine; self-hosted means Enterprise | DOC | surrealdb.com/docs/running/multi-node | 2026-09-30 | recorded |
| R-0004 | Minimum 3 nodes; odd counts preferred; even counts add no fault tolerance | DOC | surrealdb.com/docs/manage/instances/scaling | 2026-09-30 | recorded |
| R-0005 | The fast commit path requires every node to agree, so one slow node forces the slow path | DOC | surrealdb.com/docs/manage/instances/scaling | 2026-09-30 | recorded |
| R-0006 | Commit requires a quorum of acknowledgements; no elected leader | DOC | surrealdb.com/platform/surrealds | 2026-09-30 | recorded |
| R-0007 | Each availability zone runs its own write node; every write node can coordinate | DOC | surrealdb.com/blog/introducing-scale-surrealdb-cloud-built-for-high-availability-and-scale | 2026-07-02 | recorded |
| R-0008 | Consensus is embedded in the database nodes; no external coordination service | DOC | surrealdb.com/blog/introducing-scale-surrealdb-cloud-built-for-high-availability-and-scale | 2026-07-02 | recorded |
| R-0009 | Recovery restores from object storage and replays the transaction log; cost follows the log delta, not dataset size | DOC | surrealdb.com/platform/surrealds | 2026-09-30 | recorded |
| R-0010 | A distributed backend with a non-linear commit log must report a genuine *closed* watermark from `safe_timestamp`, or the live-query router misses notifications | SRC | upstream `Transactable::safe_timestamp` doc comment — interface contract | 2026-09-30 | recorded, re-verify at v3.3.0 |
| R-0011 | An indeterminate commit must be reported distinctly from a definite failure, instructing the client to read back rather than replay | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0012 | Write conflicts must be retryable and distinguishable from definite failures, so SDK retry helpers work | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0013 | Replicas holding contradictory outcomes for one transaction must converge and log both | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0014 | A transaction in flight across a membership change must rejoin the new membership or fail retryably, never stall indefinitely | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0015 | A membership-changing leader must not serve until a quorum of the new voter set holds the decided configuration | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0016 | Recovery drain must be bounded — never O(all transactions ever committed) | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0017 | Transaction write sets are bounded by both an operation count and a byte cap; over-bound fails fast at the coordinator | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0018 | Cold-start convergence must be bounded; upstream reduced worst case 86.9s → 13.7s in one release | REL | surrealdb.com/releases/3.3, SurrealDS section | 2026-09-30 | recorded |
| R-0019 | Transaction commit log is per-zone, replicated, with async durability to object storage | DOC | surrealdb.com/platform/surrealds | 2026-09-30 | recorded |
| R-0020 | Node recovery time is independent of dataset size | DOC | surrealdb.com/platform/surrealds | 2026-09-30 | recorded |
| R-0021 | Object storage is the durable tier; no separate backup infrastructure required | DOC | surrealdb.com/platform/surrealds | 2026-09-30 | recorded |
| R-0022 | A backend registers itself by claiming path schemes in a registry consulted before first-party engines | PUBAPI | `surrealdb-kvs-any` `BackendProvider::schemes` | 2026-10-01 | recorded |
| R-0023 | `ConnectContext` supplies the matched scheme, normalised path, a cancellation token, and config with URL query parameters merged under `datastore_`-prefixed keys | PUBAPI | `surrealdb-kvs-any` `ConnectContext` | 2026-10-01 | recorded |
| R-0024 | A provider may perform arbitrarily heavy async startup; the returned builder must be ready for use | PUBAPI | `surrealdb-kvs-any` `BackendProvider::connect` | 2026-10-01 | recorded |
| R-0025 | Every KV backend — first-party or external — runs the same contract suite | PUBAPI | `surrealdb-kvs-test` crate docs | 2026-10-01 | recorded |
| R-0026 | `TransactionBuilder` requires `name()` and takes a `TransactionType` enum, not boolean flags | PUBAPI | `surrealdb-kvs` 3.3.0 `builder.rs` | 2026-10-01 | recorded, implemented |
| R-0027 | `Transactable` at v3.3.0 has 36 methods, including `getu` (locked read) and `compact` | PUBAPI | `surrealdb-kvs` 3.3.0 `api.rs` | 2026-10-01 | recorded, implemented |
| R-0028 | Scan cursors return zero-copy batches built from concatenated buffers plus spans, via `KeysBatch::from_parts` / `ValsBatch::from_parts` | PUBAPI | `surrealdb-kvs` 3.3.0 `api.rs` | 2026-10-01 | recorded, implemented |
| R-0029 | A zero `limit` on a cursor call is a pure no-op: it consumes no rows, does not exhaust the cursor, and the cursor stays valid | PUBAPI | `surrealdb-kvs` 3.3.0 `ScanCursorKeys::next_batch` docs | 2026-10-01 | recorded, implemented |
| R-0030 | ~~`v3.3.0`'s `TransactionBuilder` has no `extension()` hook; the pre-release tree had one~~ — **superseded by R-0031**, this was wrong | PUBAPI | `surrealdb-kvs` 3.3.0 `builder.rs` | 2026-10-01 | **superseded** 2026-10-02 |
| R-0031 | `TransactionBuilder` at v3.3.0 has two defaulted hooks beyond the five required methods: `wait_until_serve_ready()` (unbounded wait for a backend that must join a cluster before serving) and `extension(TypeId)` returning `None` by default | PUBAPI | `surrealdb-kvs` 3.3.0 `builder.rs` | 2026-10-02 | recorded |
| R-0032 | `DestroyRangeHandle` is the shared-`TypeId` shape for out-of-transaction range destruction; a backend publishes one only while it can perform the operation, and `None` is the caller's signal to take a transactional fallback | PUBAPI | `surrealdb-kvs` 3.3.0 `destroy.rs` | 2026-10-02 | recorded, relied on |
| R-0033 | The conformance suite's backend-name vocabulary already contains `surrealds`; registering under that name puts a backend behind the suite's assertions for the distributed store and *ignores* the ones that contradict its documented model | SRC | `vendor/surrealdb-kvs-test/src/{multi,snapshot,builder_surface,raw}.rs`, `kvs_test!` `only`/`except` lists | 2026-10-02 | recorded, adopted |
| R-0034 | For a backend named `surrealds` the suite requires: overlapping blind writes to one key all commit and the last committer wins (no write-write conflict detection), reads are validated at commit so write skew is prevented, `getu` is refused with `UnsupportedLockedReads`, and `register_metrics` must return a non-empty set whose every name is collectable | SRC | `vendor/surrealdb-kvs-test/src/multi.rs` `multiwriter_same_keys_allow`; `src/snapshot.rs` `write_skew_permitted`; `src/raw.rs` `getu_unsupported`; `src/builder_surface.rs` `metrics_collectable` | 2026-10-02 | recorded, implemented |
| R-0035 | `put` refuses an existing key; a `None` precondition on `putc`/`delc`/`clrc` asserts the key is **absent**, which is what makes a conditional create atomic on every backend including last-writer-wins ones | SRC | `vendor/surrealdb-kvs-test/src/raw.rs` `put`; `src/defaults.rs` `clrc`; `src/multi.rs` `multiwriter_same_keys_putc` | 2026-10-02 | recorded, implemented |
| R-0036 | `surrealdb_server::init` takes a single generic composer implementing `TransactionBuilderFactory + RouterFactory + ConfigCheck + ObservabilityProvider`; `TransactionBuilderFactory` is the seam that accepts a caller-built `Backends` registry, and `CommunityComposer` satisfies it by delegating to `Backends::community()` | PUBAPI | `surrealdb-server` 3.3.0 `lib.rs`; `surrealdb-core` 3.3.0 `kvs/ds.rs` | 2026-10-02 | recorded |
| R-0037 | `TransactionBuilderFactory` carries the clustered-deployment hooks: `datastore_node_id()` for live-query ownership, `live_query_broker()` to relay notifications off-node instead of using the local broker, and `http_endpoint()` so the node records its own endpoint on the `Node` catalog row for peer discovery | PUBAPI | `surrealdb-core` 3.3.0 `kvs/ds.rs` `TransactionBuilderFactory` | 2026-10-02 | recorded |
| R-0039 | `RouterFactory` has one method, `configure_router`, and `community_router(exclude)` is public — it yields the community route set (`/health`, `/ready`, `/version`, `/rpc`, `/export`, `/import`, `/status`, `/grpc`, `/sync`). `community_router(&["/ready"])` is the documented seam for an edition that layers its own serve-readiness | PUBAPI | `surrealdb-server` 3.3.0 `src/ntw/mod.rs` | 2026-10-02 | recorded, used |
| R-0040 | `CommunityComposer` is a public unit struct implementing all four composer traits; `surrealdb_server`'s `ObservabilityProvider` impl for it is empty, so `create_observer` is the only method an embedder must supply | PUBAPI | `surrealdb-core` 3.3.0 `lib.rs`; `surrealdb-server` 3.3.0 `src/observe/provider.rs` | 2026-10-02 | recorded, used |
| R-0041 | `surrealdb_server::init` boots the upstream CLI over `std::env::args()`, so an embedding binary inherits the `surreal` command surface (`start`, `version`, `config`, …) rather than exposing a bespoke one; it also performs an online version check at startup unless `--online-version-check=false` / `SURREAL_ONLINE_VERSION_CHECK=false` | PUBAPI | `surrealdb-server` 3.3.0 `src/lib.rs`, `src/cli/mod.rs` | 2026-10-02 | recorded, verified |
| R-0042 | `/rpc` with `Content-Type: application/json` requires a JSON-RPC **object** body — the query goes in `params`, not as a bare SurrealQL string. There is no auto-create of namespaces or databases, and `RETURN` is evaluated without resolving the database, so it returns OK against any database name | PUBAPI | `surrealdb-server` 3.3.0 `src/rpc/format.rs`, `src/ntw/headers/content_type.rs`; verified against a running server | 2026-10-02 | recorded, verified |
| R-0043 | A live transaction must pin its snapshot stamp; collection may then retain only the newest version at or below the oldest pinned stamp, because for any live snapshot at or above the horizon every older version is shadowed by it | SRC | `crates/surrealdb-ds/src/storage.rs` module docs; sufficiency argument recorded in-file | 2026-10-02 | recorded, implemented, tested |
| R-0038 | A relay broker and the endpoint resolver meet **after** the datastore is built, not at composer time: `dbs::NodeEndpointResolver` (`resolve(target_node) -> Option<String>`, catalog-backed) is handed to the broker through `dbs::BrokerRoutingContext`, alongside the local node id the broker uses to skip delivering to itself | PUBAPI | `surrealdb-core` 3.3.0 `dbs/broker.rs` | 2026-10-02 | recorded |
| R-0044 | `surrealdb-kvs`'s `key` and `value` modules declare a *mechanism*, not a layout: `KVKey::encode_buffer` / `KVKeyDecode` / `KVValue::kv_encode_value` are traits each key and value type implements, and nothing in the crate says what a namespace or an index is. The engine's keyspace is declared one level up, by a single `keyspace!` invocation in `surrealdb-datastore` | PUBAPI | `surrealdb-kvs` 3.3.0 `src/lib.rs`, `src/key/mod.rs`; `surrealdb-datastore` 3.3.0 `src/key/mod.rs`, `src/key/schema.rs`, `src/key/keyspace.map` | 2026-10-04 | recorded, relied on |
| R-0045 | `Transactable` at v3.3.0 is **byte-level**: `set(key: Key, val: Val)`, `get(key, version) -> Option<Val>`, `scan(KeyRange, …) -> Vec<(Vec<u8>, Val)>`. A key `is` a `Cow<[u8]>` (`Key<'a>`), so the query layer above the seam hands a backend already-encoded bytes and the backend never decodes. Byte compatibility is therefore faithful passthrough, not a re-implementation of an encoder | PUBAPI | `surrealdb-kvs` 3.3.0 `src/api.rs`, `src/types.rs` | 2026-10-04 | recorded, load-bearing for L2 |
| R-0046 | A datastore can be built in-process against **any** `Backends` registry without `surrealdb_server::init`: `surrealdb_server` re-exports `surrealdb_core` as `surrealdb_server::core`, and `core::kvs::Builder::build_with_factory_path(path, composer)` takes a `TransactionBuilderFactory` — the same seam `init` uses. No argv, no global tracing subscriber, no online version check, no socket. `Datastore::execute` runs SurrealQL and `Datastore::transaction` opens a `Transaction` whose `scan_raw` reads a `RawRange` as `(key, bytes)` pairs | PUBAPI | `surrealdb-server` 3.3.0 `src/lib.rs:68`; `surrealdb-core` 3.3.0 `src/kvs/ds/builder.rs`, `src/kvs/ds.rs`; `surrealdb-datastore` 3.3.0 `src/tx.rs` `scan_raw` | 2026-10-04 | recorded, used |
| R-0047 | `new_transaction_builder` is what **opens** a store: `ds+mem://` constructs a fresh in-memory tier per call, so writing and then reading back through two calls compares a populated store against an empty one. Anything that must see its own writes has to hold one `TransactionBuilder` for both | PUBAPI | `surrealdb-kvs-any` 3.3.0 `Backends::new_transaction_builder`; observed in `crates/surrealdb-ds/src/builder.rs` `DsTransactionBuilder::connect` | 2026-10-04 | recorded, verified by the golden harness |
| R-0048 | `surrealdb-kvs-rocksdb` is **unversioned by default** (`RocksDbConfig::versioned = false`), so a plain `rocksdb:` path installs neither the `surrealdb.TimestampComparator` user-defined-timestamp comparator nor `set_timestamp(u64::MAX)`. Verified in the `OPTIONS` file a plain construction writes: `comparator=leveldb.BytewiseComparator`. The UDT layout applies only to `?datastore_versioned=true` | PUBAPI | `surrealdb-kvs-rocksdb` 3.3.0 `src/cnf.rs` `RocksDbConfig::default`, `Config::parse` (`datastore_versioned`); `src/comparator.rs` | 2026-10-04 | recorded, verified |
| R-0049 | A datastore built through the KV contract and left on its defaults refuses versioned reads: `surrealdb-kvs-mem` rejects `datastore_versioned` at startup with `Error::UnsupportedVersionedQueries`, and both mem and rocksdb default it off. Our tier's refusal of a `version` argument is therefore upstream's own position for a backend registered without versioning, not a gap we invented | PUBAPI | `surrealdb-kvs-mem` 3.3.0 `src/lib.rs` `Datastore::new`; `src/cnf.rs` | 2026-10-04 | recorded |

---

## Gaps

Requirements we know we need but have not yet sourced. Filling these requires
black-box observation (`OBS`), which has not started.

| Gap | Why it matters | Planned phase |
| --- | --- | --- |
| Exact key encoding for every record class | Required for L2 byte compatibility | 1 |
| Wire shape of the `Key`/`Val` byte contract for graphs, indexes, vectors, changefeeds | Required for L2 | 1 |
| Precise conflict-detection window: what conflicts with what | Silent data corruption if wrong | 2 |
| `safe_timestamp` semantics under partial commit visibility | Live-query correctness | 2 |
| Read-your-writes guarantees across nodes after a quorum commit | Stale reads would break SurrealQL semantics | 3 |
| Behaviour under clock skew | HLC-based timestamps are advertised | 3 |

### Closed by the golden-file harness, and how far

The first two gaps are **no longer gaps in the sense that mattered**, and it is
worth being precise about what changed. They were written as gaps because the
encoding was unknown. It is not unknown: `surrealdb-datastore`'s `keyspace!`
invocation declares it (R-0044), and the layer above the seam hands a backend
already-encoded bytes (R-0045), so **we never need to know it**. What Phase 1 had
instead was no *evidence* that the bytes we hold are the bytes a real SurrealDB
node holds.

`crates/surrealdb-ds-server/tests/golden.rs` is that evidence, and it is
`make golden`:

- A real `Datastore` runs real SurrealQL against upstream's own `rocksdb:`
  (R-0046), the whole keyspace is dumped, and those exact bytes are replayed
  through `ds+mem://` and read back. Every key and every value is compared, with
  no exclusions — the round trip compares bytes that were *copied*, not bytes a
  run happened to *generate*.
- The dataset covers graphs (edge documents plus the `~` keys on both endpoints),
  a unique index, an HNSW vector index and its serialised vectors, record
  documents, doc-id mappings, field and index definitions, and a tombstone.
- A third store replays our output back into upstream's engine, so a failure is
  attributable to our tier rather than to the replay mechanism.

Two things it explicitly does **not** establish, recorded here so a green run is
not read as more than it is:

1. **No key class is proven complete.** The harness compares the classes this
   workload happens to produce. A class upstream writes that this workload never
   reaches is untested, and no failure would announce it.
2. **The committed manifest pins 35 of the dataset's 55 keys**, not all of them.
   The other 20 carry a fresh `Uuid::new_v7()` per run — node rows, id-sequence
   state, index-build state, and the table definitions, which is where
   `PERMISSIONS` lives. Those bytes *are* compared, absolutely, by the round trip;
   they are simply not in the committed file, because no byte-pinning of a
   value containing a millisecond timestamp survives a day.

---

## How to add a record

1. Assign the next `R-NNNN`.
2. State the requirement as a testable assertion, not a paraphrase.
3. Record the source class and a precise citation with an access date.
4. If it came from `OBS`, attach the reproduction: instance type, version,
   commands run, observed output.
5. Reference the `R-NNNN` from the relevant file in `docs/spec/`.

If a requirement's source is ever unclear, it does not go in the spec. Finding
out *how you know* is the entire point of this file.