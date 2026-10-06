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
| R-0050 | `surrealdb.TablePrefix.v1` extracts the prefix of a *table-level* key, variable-length, ending **at and including** a one-byte discriminator. A key is in the domain iff it is at least 14 bytes, has `/`, `*`, `*`, `*` at offsets 0, 1, 6, 11 (`/`, `*`, a 4-byte namespace id, `*`, a 4-byte database id, `*`), has a `\0` at or after offset 12, and has at least one byte after that `\0`. The prefix is `key[..null_pos + 2]`. Discriminators: `*` records, `+` index entries, `!` table metadata, `~` graph edges, `&` refs. Out-of-domain keys are returned **unchanged** by `Transform` and excluded by `InDomain`, which is what keeps an unrecognised key out of the bloom filter | SRC | `surrealdb-kvs-rocksdb` 3.3.0 `src/prefix_extractor.rs` `parse_prefix_end` / `transform` / `in_domain`, `NAME`, `TB_START`, `MIN_LEN` — format shape only, reimplemented in `crates/surrealdb-ds-server/tests/interop.rs` from the documented layout | 2026-10-04 | recorded, implemented, verified against 55 real keys |
| R-0051 | **The recorded `prefix_extractor` name is not what admits the open.** `DB::open` does not read the on-disk `OPTIONS` file — the options in force are those passed in code, and the `prefix_extractor` line is consulted only by an explicit `Options::load_from_file`. Measured: the same upstream-written directory opens and reads all 55 keys byte-identically with **no** extractor registered, and with one registered as `wrong.Name.v0`. This **supersedes the premise** the ADR-0008 tranche was briefed on | OBS | `cargo test -p surrealdb-ds-server --test interop the_named_extractor_is_not_what_makes_the_open_succeed`; corroborated by the absence of any `OPTIONS`-load step on the `DB::open` path in `surrealdb-rocksdb` 0.24.0-surreal.5 `src/db.rs` `DB::open` | 2026-10-04 | recorded, verified; **refines ADR-0008** |
| R-0052 | The extractor's *computation* is nevertheless load-bearing, and a wrong one **fails silently**: under `ReadOptions::set_prefix_same_as_start(true)` the same prefix-restricted seek returns 2 rows with the correct extractor and 12 with a mismatched one or none — a wider answer, never an error. On a full forward scan the mismatch is invisible, so byte-equality against a whole keyspace does **not** validate the extractor | OBS | `cargo test -p surrealdb-ds-server --test interop a_mismatched_extractor_silently_widens_a_prefix_restricted_read`; seek key `/*\0\0\0\0*\0\0\0\0*person\0*\0`, upstream-written directory, 55-key dataset | 2026-10-04 | recorded, verified |
| R-0053 | A clean shutdown **rewrites the node row**: upstream archives its own `/!nd{nd}` entry on the way out, so `Node.gc` goes `false -> true` and byte 27 of that 29-byte value changes. Measured: 1 of 55 keys differs between a keyspace dumped through a live `Datastore` and the same directory read after `shutdown()`. A harness that dumps through a live writer and then reads the directory is comparing two different moments | OBS | `cargo test -p surrealdb-ds-server --test interop a_clean_shutdown_rewrites_the_node_row`; `surrealdb-catalog` 3.3.0 `src/node.rs` `Node { id, heartbeat, gc, http_endpoint }` — field order, for reading the byte | 2026-10-04 | recorded, verified |
| R-0054 | The RocksDB binding upstream uses is the SurrealDB-maintained fork `surrealdb-rocksdb` **0.24.0-surreal.5** over `surrealdb-librocksdb-sys` **0.18.3+11.0.0-4** (RocksDB **11.0.0**), requested with features `lz4` + `snappy` only. The public `rocksdb` crate's newest line resolves to `librocksdb-sys` 0.19.0 = RocksDB **11.8.1**, and its 0.24.0 line to 0.17.3 = RocksDB **10.4.2**. `format_version=7` is readable by all three (`kLatestFormatVersion`/`kLatestBbtFormatVersion = 7`, read floor 2), so **format_version does not discriminate between them** | PUBAPI | crates.io sparse index `ro/ck/rocksdb` and `li/br/librocksdb-sys`, read 2026-10-04; `surrealdb-kvs-rocksdb` 3.3.0 `Cargo.toml` `[dependencies.rocksdb]`; `table/format.h` in each tree | 2026-10-04 | recorded, verified |
| R-0055 | A directory written by our own `rocksdb` binding with **no prefix extractor**, one column family (RocksDB's default, the only one upstream uses), explicit key bytes, flushed to SSTs and closed, is opened by `surrealdb-kvs-rocksdb`'s reader; the whole live keyspace reads back byte-identically (7 keys / 108 value bytes, 1 tombstone, 2 SSTs). A point read returns the right value and the tombstoned key reads absent. No special column family or on-disk marker is needed for upstream's `OptimisticTransactionDB` path to open a plain `DB` directory | OBS | `make interop` — `cargo test -p surrealdb-ds-server --test interop we_write_a_directory_upstream_can_open_and_read`; 2026-10-05 | recorded, verified |
| R-0056 | Bounded range reads through upstream's reader over a directory we wrote with **no prefix extractor** return **exactly** the bounded slice of the full keyspace — 2 record rows, 1 index, 1 metadata, 1 edge, 1 edge-document, 5 cross-category, 1 root row — with no silently widened or dropped rows. This covers both modes upstream selects: bounds that are in-domain and share an extracted prefix, and bounds that differ or are out of domain. Our own reader under explicit lower/upper bounds returns the same slices | OBS | `make interop` — same test as R-0055; `src/lib.rs` `scan_read_options` / `apply_prefix_mode` | 2026-10-05 | recorded, verified |
| R-0057 | `surrealdb-kvs-rocksdb` chooses the scan prefix mode **per range**: `ReadOptions::set_prefix_same_as_start(true)` when *both* bounds are in the extractor's domain and extract to the same prefix, else `set_total_order_seek(true)`; explicit iterate lower/upper bounds are set in both cases. On open it installs `TablePrefix.v1` because `RocksDbConfig::prefix_extractor_enabled` defaults to `true`. A file written with no extractor is therefore still read under a prefix-restricted path | SRC | `surrealdb-kvs-rocksdb` 3.3.0 `src/lib.rs` `apply_prefix_mode` (L970–991), `scan_read_options` (L996–1016); `src/cnf.rs` `RocksDbConfig::default` `prefix_extractor_enabled: true` (L644) — interaction shape only, no code copied | 2026-10-05 | recorded, verified |
| R-0058 | After upstream's reader writes one key and deletes another through a directory we wrote, both our reader and upstream's reader agree byte-identically on the net effect: the new key present, the deleted key absent, and the tombstone the writer already left still absent | OBS | `make interop` — same test as R-0055 (steps 3–5) | 2026-10-05 | recorded, verified |
| R-0059 | A keyspace written through our own `surrealdb-rocksdb` binding, **closed cleanly**, and reopened reads back byte-identically — and identically across a *second* reopen, so the first reopen may itself have left a WAL. A point read returns the right value. This is the baseline every crash property is judged against: a clean close that is not faithful makes a crash result meaningless | OBS | `make durability` — `cargo test -p surrealdb-ds-server --test durability a_clean_close_reopens_byte_identically`; 16-key payload | 2026-10-06 | recorded, verified |
| R-0060 | With `WriteOptions::sync = true`, a commit acknowledged immediately before `SIGKILL` is recovered in full from the **WAL**, not from an SST: the child leaves 0 `.sst` files and a non-empty `.log`, and a plain `DB::open` replays it. The SST count is checked **before** the parent opens, because RocksDB's own recovery flushes the replayed memtable to an L0 SST as it opens, which makes "no SST" meaningless afterwards | OBS | `make durability` — same test; `surrealdb-librocksdb-sys` 0.18.3+11.0.0-4 `db/db_impl/db_impl_write.cc` L1389–1398 — when `sync` is set the write path `FlushWAL(true)` under `manual_wal_flush_`, else `SyncWAL()` | 2026-10-06 | recorded, verified |
| R-0061 | **`SIGKILL` does not simulate power loss, and cannot.** The kernel page cache outlives the process, so a `sync = false` commit survives: measured **600 of 600** process kills returned the full commit, 0 lost. `sync = true` and `sync = false` are therefore *indistinguishable* to a kill-based harness. That `sync = true` survives media loss is documented behaviour, **not measured here** — nothing in this test can drop the page cache. A durability harness built only on `SIGKILL` proves recovery, not durability | OBS | `make durability` — `an_unsynced_commit_is_measured_not_assumed`; 30 runs × 20 trials | 2026-10-06 | recorded, verified; **narrows the step-2 brief's premise** |
| R-0062 | `Options::manual_wal_flush = true` with `sync = false` holds the WAL record in the application, so a killed writer usually loses the commit — measured **576 of 600 lost** — but it is **not a barrier**: the engine flushed the buffer on its own schedule often enough that **24 of 600 survived** a `SIGKILL` landing microseconds after the commit was acknowledged. The child reports 0 WAL bytes at commit time and the parent finds the full 403-byte record (7-byte WAL header + 396-byte batch) on disk after the kill, with no memtable flush in the child's own info log. **Only `sync` is a caller-held durability barrier**; "I did not fsync" is not the same claim as "this will not be on disk" | OBS | `make durability` — `manual_wal_flush_loses_the_unsynced_commit_but_not_always`; 30 runs × 20 trials; corroborating code: `db/log_writer.cc` `Writer::AddRecord` L189–193 — the buffer is flushed only when `!manual_flush_`, and the buffer is flushed automatically when it cannot fit a write (`file/writable_file_writer.cc` `Append` L104–118) | 2026-10-06 | recorded, verified; **negative result, load-bearing for step 3** |
| R-0063 | `WriteOptions::disable_wal = true` removes the durable path: measured **595 of 600 lost** to `SIGKILL`, the surviving 5 attributable to a memtable flush the engine chose on its own (the only route a WAL-less write reaches an SST). The WAL is therefore the mechanism, not a convenience — and its absence is probabilistic, not an error, so a caller cannot treat a disabled WAL as "fails loudly" | OBS | `make durability` — `disabling_the_wal_loses_the_commit_but_not_always`; 30 runs × 20 trials | 2026-10-06 | recorded, verified; **negative result** |
| R-0064 | An explicit `DB::flush_wal(true)` after a `manual_wal_flush` commit moves the buffered record and the commit is recovered in full, with no `sync`. So `flush_wal` and `sync` are distinct operations: the first is a caller-invoked barrier (the buffer is out of the application), the second is the fsync the caller holds per write | OBS | `make durability` — `an_explicit_wal_flush_buys_back_the_unsynced_commit` | 2026-10-06 | recorded, verified |
| R-0065 | A multi-key `WriteBatch` is **all-or-nothing** across a crash, on two independent legs: (a) a child looping 400-key batches (≈110 KiB, several WAL blocks each) and killed mid-write left **every** batch complete — 118 batches / 47 200 keys in the recorded run, batch ids sequential with no gap, 0 SSTs left by the child; (b) deterministically, truncating the last 256 bytes of a 111 240-byte WAL record recovers **0 keys, not 400** — a torn record is dropped whole. A partial batch is the failure the property forbids and the harness bails on one | OBS | `make durability` — `a_killed_writer_never_leaves_a_partial_batch`, `a_torn_wal_record_is_dropped_not_half_applied`; the truncation is on the WAL only, and the directory is deliberately **not** opened before the tear because recovery would flush a replayed memtable to an SST and mask the result | 2026-10-06 | recorded, verified |
| R-0066 | Deletes survive a crash **and** a compaction in both placements: the tombstone in the WAL, and the tombstone flushed to its own SST so compaction must merge it away. Measured: 24 live keys intact and 8 deleted keys absent after recovery, again after `flush()` + `compact_range(None, None)`, and again after a second reopen. Compaction is where a resurrected value would appear and nowhere earlier | OBS | `make durability` — `tombstones_survive_a_crash_and_a_compaction` | 2026-10-06 | recorded, verified |
| R-0067 | A commit made through `OptimisticTransactionDB` with `sync = true` is durable under `SIGKILL`, and a **plain `DB`** recovers its directory — upstream's transaction path and our reader agree without any on-disk marker | OBS | `make durability` — `an_optimistic_commit_is_durable_under_sigkill`; the reverse direction is R-0055 | 2026-10-06 | recorded, verified |
| R-0068 | **`OptimisticTransactionDB` rejects overlapping blind writes to one key** — measured, not assumed: of two concurrent transactions both blind-putting one key, the first commit succeeds and the second **fails**, with or without `OptimisticTransactionOptions::set_snapshot(true)`, leaving the first writer's value. This is the `surrealds` contract inverted (R-0034: overlapping blind writes must all commit, serialised in stamp order), so RocksDB's transaction API cannot be used naively as the durable tier's commit path — it fails conformance for the right-looking reason | OBS | `make durability` — `optimistic_overlapping_blind_writes_are_measured`; documented behaviour in `surrealdb-rocksdb` 0.24.0-surreal.5 `src/transactions/transaction.rs` `commit` (L141) — a `TryAgain` may be returned if the memtable history no longer covers the write set | 2026-10-06 | recorded, verified; **confirms ADR-0012's stated risk** |

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

## Closed by the interop tranche (ADR-0008), and how far

`crates/surrealdb-ds-server/tests/interop.rs` is `make interop`, and it is the
falsification experiment ADR-0008's own Sequencing section asked for: open a
directory upstream wrote with `rocksdb` as **our** dependency, register a
`SliceTransform` named `surrealdb.TablePrefix.v1`, and read the keyspace back byte
identically. It passed on the first correct attempt — 55 keys, 1526 value bytes,
every key and every value, read both by our binding and by upstream's own reader
on the same closed directory.

It also **narrowed** the decision in two places worth more than the pass:

- **R-0051 supersedes the brief's premise.** The recorded `prefix_extractor` name
  is not what admits the open; `DB::open` never reads the on-disk `OPTIONS` file.
  The directory opens with no extractor registered, and with one registered under
  a name upstream never wrote. Anyone budgeting the tier for "reproduce a name
  RocksDB resolves on open" was budgeting for the wrong thing.
- **R-0052 replaces it with a sharper obligation.** The extractor's *computation*
  is load-bearing and a wrong one fails silently rather than loudly, returning a
  **wider** range than asked for under `prefix_same_as_start`. That obligation is
  invisible to a whole-keyspace byte comparison, which is why two tests exist that
  do not compare bytes at all.

And it recorded one upstream behaviour nobody had written down (**R-0053**): a
clean shutdown archives upstream's own node row, so a keyspace dumped through a
live `Datastore` is not the keyspace on disk afterwards. The first version of the
harness compared exactly those two and failed on exactly one byte, reproducibly.
That is a trap for the tier as much as for the harness.

What it does **not** establish, so that a green run is not read as more than it is:

1. **Nothing about writing.** Every test here reads a directory upstream wrote.
   The other half of ADR-0008 — a directory *we* wrote that upstream opens — is
   untested, and it is the half that needs the `OPTIONS` profile reproduced rather
   than merely satisfied on read.
2. **No key class is proven complete**, the same limit the golden harness records.
3. **A 55-key dataset never leaves one level.** Compression, blob files,
   two-level indexes and partitioned filters are *configured* in the `OPTIONS`
   file and *exercised* nowhere. `upstream_leaves_sst_files_behind` asserts the one
   thing that is checked about it — that a real SST is read, not only a WAL — but
   one SST of one level is not a format.
4. **The `OPTIONS` file records settings, not behaviour.** It names codecs the
   linked binding does not compile (`bottommost_compression=kZSTD`, with only
   `lz4` and `snappy` built), and nothing has ever failed, because the compaction
   that would have used them never ran. It is a capture of upstream's *defaults
   plus its overrides*, and it is wrong about at least one thing that matters for
   writing: `compression_per_level` is `kNoCompression:kLZ4×4:kZSTD×3`, hardcoded
   upstream, and a writer that does not reproduce it produces files upstream can
   still read — but not files upstream would have produced.

---

## Closed by the write probe (ADR-0012 step 1), and how far

`crates/surrealdb-ds-server/tests/interop.rs` now also writes a small keyspace
with `rocksdb` as **our** dependency — no prefix extractor, one default column
family, explicit bounds, two flushed SSTs, one tombstone — and hands it to
upstream's reader. The whole keyspace reads back byte-identically, the bounded
ranges return exactly the bounded slice (the failure ADR-0011 showed is silent),
the point reads are right, and a write/delete round trip through upstream leaves
both readers agreeing on the net effect. Records: **R-0055** through **R-0058**.

Two things are worth separating, because the pass is not the interesting part:

- **The open is not the result; the ranges are.** ADR-0011 established that a
  wrong prefix extractor widens a prefix-restricted read without an error, and
  that byte equality cannot see it. So the writer installs no extractor at all
  and the probe asserts bounded ranges against the bounded slice of the full
  read — including a range whose ends share an in-domain prefix, the path on
  which upstream's reader enables `prefix_same_as_start` (R-0057). That is the
  assertion a wrong filter would break.
- **`OPTIONS` is not the spec.** Our directory records no prefix extractor;
  upstream still opens it, because `DB::open` never reads the on-disk `OPTIONS`
  file (R-0051). The directory that step 6's export profile must produce is a
  separate, stricter thing than the directory this probe proves is readable.

What it does **not** establish, so a green run is not read as more than it is:

1. **No tier.** Raw RocksDB, no WAL recovery, no commit semantics, no reopen
   after a crash. ADR-0012 steps 2–6 are the tier; this is the one-day spike.
2. **No key class is proven complete**, the same limit the golden and interop
   read halves record.
3. **One SST per flush and no compaction**, so compression, blob files,
   two-level indexes and partitioned filters remain configured-but-unexercised.
4. **The falsifiability is on the comparison, not on upsets to the extractor.**
   A flipped bit in what upstream hands back is caught and named
   (`the_write_probe_comparison_catches_a_flipped_bit`); the widening class
   itself is demonstrated on upstream's directory by
   `a_mismatched_extractor_silently_widens_a_prefix_restricted_read`.

---

## Closed by the durability probe (ADR-0012 step 2), and how far

`crates/surrealdb-ds-server/tests/durability.rs` is a measurement harness, not a
tier. It re-execs the test binary as a crash writer (`DS_DURABILITY_CHILD`), lets
it commit, and `SIGKILL`s it, because an in-process panic runs `Drop` and proves
nothing about recovery. Records: **R-0059** through **R-0068**.

The result is a list of properties with the configuration each one needs, and the
configuration is the point:

- **Holds with configuration:** per-commit durability needs `WriteOptions::sync`.
  Atomic batches, tombstones, and clean reopen hold on the defaults.
- **Holds without configuration:** nothing about durability does. `sync` is off
  by default, and the harness's own measurement shows the default commit survives
  a kill for the wrong reason (R-0061).
- **Does not hold:** `manual_wal_flush` and `disable_wal` are *not* durability
  barriers — they lose the commit usually (576/600, 595/600) and survive it
  occasionally (24/600, 5/600) with no error anywhere (R-0062, R-0063). A caller
  cannot rely on either as a deliberate barrier.

What it does **not** establish:

1. **Power loss.** Everything here is `SIGKILL`, which preserves the page cache.
   The `sync` guarantee that matters is documented, not measured (R-0061).
2. **A tier.** No `Transactable`, no read validation, no commit identity.
3. **Distributed commit identity.** A local sequence number is not a cluster-wide
   timestamp (ADR-0012); nothing here tests one.
4. **Stable rates.** The two "does not hold" rates are timing-dependent under
   parallel libtest, which is exactly why they are reported as counts over many
   process kills rather than as verdicts.

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