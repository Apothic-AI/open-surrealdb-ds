# SurrealDB self-hosting + horizontal scaling — research notes

- **Researched:** 2026-10-01
- **Current stable release at time of research:** `v3.3.0` (released 2026-09-24; 5 patch releases on the 3.3 line, all patches folded into 3.3.0)
- **Previous major for comparison:** `v3.0.x` line ended at 3.0.5 (2026-03-27); 3.1 introduced Enterprise/SurrealDS work; 3.2 preceded 3.3
- **Scope:** what it takes to self-host SurrealDB today, and what it takes to scale it horizontally
- **Method:** official docs (`surrealdb.com/docs`, plus `llms.txt` index and the 7.1 MB `llms-full.txt` corpus), release notes, marketing pages, and pricing page. No hands-on cluster was built; no Enterprise licence obtained.

---

## 1. TL;DR

1. **Community Edition is now a single-node product.** RocksDB on one host, vertical scaling only, no built-in HA.
2. **Horizontal scaling is commercial.** It requires the SurrealDS distributed storage engine, which self-hosted requires a SurrealDB Enterprise licence (contact-sales pricing) and Kubernetes.
3. **SurrealDS is very thinly documented.** No reference page exists in the docs index at all. The entire 7.1 MB docs corpus mentions "SurrealDS" 15 times. Public material = a marketing page, ~30 metric names, 8 tuning env vars, and release-note bug descriptions that leak the protocol.
4. **The complete configuration reference is contract-gated.** The public env-var page says so explicitly: *"The complete `SURREAL_DS_*` reference is part of the Enterprise Kubernetes deployment guide."*
5. **No free/eval path exists** to run SurrealDS yourself. No benchmarks against peer systems. Enterprise is a separate closed binary, not open source.

---

## 2. Deployment model matrix (as documented)

| Deployment model | Storage engine(s) | Scaling | HA | Versioning (`VERSION`) | Managed option |
|---|---|---|---|---|---|
| Managed — Start | single node | vertical | no | where enabled | Cloud Start |
| Managed — Scale | SurrealDS (multi-node cluster) | vertical + horizontal | yes, quorum | where enabled on storage tier | Cloud Scale |
| Single node (self-hosted) | RocksDB (recommended for servers), SurrealKV (beta) | vertical | none | where enabled | Community Edition |
| Multi-node (self-hosted) | SurrealDS | horizontal | yes | where enabled | **Enterprise (licensed)** |
| Embedded | SurrealMX (memory), SurrealKV, RocksDB, IndexedDB | process-bound | process-bound | where enabled | n/a |

SurrealDB separates the query engine (compute) from the storage layer. Same SurrealQL / APIs / SDKs across embedded, single node, cluster, managed.

### Storage engine status
- **RocksDB** — the conservative production pick for on-disk servers. Extensive tuning surface.
- **SurrealKV** — SurrealDB's own LSM engine, **beta**, deliberately *smaller* config surface than RocksDB, aimed at embedded / local-first. Evaluate first for embedded, not for production servers.
- **SurrealMX** (`mem://`) — in-memory, lock-free MVCC, default embedded backend since 3.0. From 3.3.0 it **no longer supports `VERSION` queries** and refuses `versioned=true` / non-zero `retention` at startup.
- **TiKV (`tikv://`)** — still compiled in, but the `surreal start` reference now says: *"supported for **local multi-node experimentation** with the Community edition. Production multi-node HA uses distributed storage on SurrealDB Cloud Scale or SurrealDB Enterprise."* Zero mentions in the docs index. Treat as a dev toy.
- **FoundationDB (`fdb://`)** — **removed in 3.0.0**; only 2.x accepts it.

---

## 3. What self-hosting requires today (Community)

### Topology
One process, one RocksDB file per database. On Kubernetes the official Helm chart guidance is explicit: keep `replicaCount: 1`, `strategy: Recreate`, ReadWriteOnce PVC mounted at `/home/nonroot`. **Multiple pods must never share one RocksDB file.**

Chart: `helm repo add surrealdb https://helm.surrealdb.com`, chart `surrealdb/surrealdb`.

### Minimum viable
```
surreal start --user root --pass secret rocksdb://data/app.db
```
Docker image runs as non-root; with a named volume you must either set `user: root` or bind-mount a host dir owned by the invoking uid, else `Failed to create RocksDB directory / Permission denied`.

### Operations you own
- **Backups:** `surreal export` (logical SurrealQL dump) + `surreal import` for restore; plus volume/storage snapshots. No managed backups self-hosted. No PITR — each export is a single point in time; narrow the window with more frequent exports, replicas, or journaled storage upstream. Validate restores into non-production regularly; keep copies off-site and encrypted; direct file copies only while no process has the datastore open.
- **Monitoring:** `/metrics` Prometheus scrape, OTLP push, `/health` + `/ready` probes, audit + slow-query logs (Enterprise), `surrealdb_license_*` gauges (Enterprise, authenticated scrape only).
- **Upgrades:** stop → replace binary → start (single node), or rolling one-node-at-a-time (cluster). Take a fresh export *before* every upgrade.

---

## 4. What horizontal scaling requires now

Two paths only.

### Option A — SurrealDB Cloud Scale (managed)
- **$0.192/node/hr**, three-node minimum → ~$420/mo for 3 nodes before storage/egress. Annual discounts available on request.
- Launched Jul 2026, applies from v3.1.
- Multi-AZ, horizontal scale-out, multi-region DR.
- Still roadmap (marked "coming soon"): object-storage-backed data, database branching/forking, custom managed backups, cross-region replication, read replicas, automatic sharding.

### Option B — SurrealDB Enterprise, self-hosted
"Priced for your deployment." No public rate card. Contact sales.

Concrete requirements per docs:
- **3+ nodes minimum, and odd (3/5/7).** "An even node count does not improve fault tolerance. A cluster of N nodes survives the same number of failures as one of N−1 nodes. The fast commit path also needs every node to agree, so one slow node forces the slower path."
- **Kubernetes.** The storage-layer operator and runbooks ship with Enterprise. Community k8s path is single-pod RocksDB only.
- **Licence key**, via `SURREAL_LICENSE_KEY` or `SURREAL_LICENSE_KEY_FILE` (setting both = startup error; relative path = startup error; unreadable/empty key file = startup error).
  - Validated **online against `api.keygen.sh`** on first start; signature checked against a key compiled into the binary; revalidated every 30 min ±10% jitter.
  - `SURREAL_LICENSE_STATE_PATH` = attestation cache, absolute path, created mode `0600`, must live on a **persistent volume** (container writable layer is lost on recreate). Lets a node boot while the licence service is unreachable.
  - Offline budget is **24 hours** from the last verified response, and it's **one budget, not per-restart** (measured from keygen.sh's signed timestamp).
  - Expired = refuses to start, no expiry grace. Suspended/banned = `/ready` 503s, process exits status 3 after 24h; reinstated within that window restores service without restart. Refused start exits status 1.
  - An attestation cached by `3.3.0-beta.4` is not accepted by `3.3.0` → first start after that upgrade needs keygen.sh reachable.
- **Object storage** for the durable tier: S3 / S3-compatible, GCS, Azure Blob.
- **QUIC transport** between SurrealDS nodes.
- Rolling upgrades keep the cluster serving; capacity dips per node restart.
- **Backups remain mandatory** even with quorum: "Quorum protects against infrastructure failure, not against a mistaken `DELETE` or a bad migration. Those replicate to every node exactly as intended."

### Migration between editions
Same core binary family; no data migration, no schema or SDK change — swap the binary and supply the licence key. "The same storage backend and the same schema are read by the Enterprise binary; the licence key is what changes."

### Enterprise feature split (3.3.0)
Community now includes: S3/GCS/Azure `DEFINE BUCKET` backends, `SELECT ... FOR UPDATE` (all community backends incl. TiKV), audit-log *documentation* (audit logging itself remains Enterprise), Surrealism WASM, Postgres wire protocol.
Enterprise-only: distributed/clustered HA, horizontal compute + storage scaling, **distributed live queries**, FIPS 140-2 modules, trusted execution (coming soon), audit logging sink, BYO encryption keys, priority security patching, contractual SLAs (Standard 8h Sev-1 business hours; Business Critical 24x7 2h; Premium 24x7 30min + named TAM + dedicated Slack).

---

## 5. SurrealDS: what they actually publish

### 5.1 Documentation audit (measured 2026-10-01)

| Surface | Status |
|---|---|
| Docs index (`llms.txt`, 1066 lines) | **no SurrealDS page at all** |
| Full docs corpus (`llms-full.txt`, 7.1 MB) | 15 mentions of "SurrealDS" |
| Public env-var reference | 8 `SURREAL_DS_*` vars, with default + "tune in response to" metric |
| Metrics reference | `surrealdb.ds.*`, "around thirty instruments" |
| `surreal start` reference | **no datastore scheme** for SurrealDS |
| Dedicated SurrealDS architecture doc | does not exist publicly |
| Complete `SURREAL_DS_*` reference | "part of the Enterprise Kubernetes deployment guide" (gated) |
| Benchmarks for the distributed engine | none published (3.0 blog is single-node RocksDB/SurrealKV only) |
| Protocol spec / paper / talk | none found |
| Eval or free tier | none |

### 5.2 Architecture claims (marketing + deep-dive pages)
- Compute/storage separation; query nodes stateless, hold no data of their own. Nodes join/leave without data redistribution.
- **Quorum consensus, no elected leader.** "Each availability zone has its own write node, writes scale horizontally, and transactions commit once a quorum acknowledges." Multi-write-node — every write node can accept and coordinate transactions, so no leader bottleneck.
- Consensus embedded **directly in the SurrealDB nodes** — no external coordination service (no ZooKeeper/etcd tier).
- Single broadcast replaces multi-hop coordination → lower latency than leader-based replication.
- Two-tier durability: replicated WAL for fast node recovery, async durability to object storage for secondary-region DR. Object storage is the durable tier at "11 nines" (99.999999999%).
- Recovery = restore from object storage + replay transaction log; "recovery time is independent of dataset size" / follows log delta.
- Scale-to-zero: idle compute shuts down, data persists in object storage, restore in seconds.
- Instant branching: petabyte-scale branch in seconds as a logical reference over shared storage.

### 5.3 Protocol hints leaked by names

**Metrics** (`surrealdb.ds.*`, Enterprise, authenticated scrape only, ~30 instruments) covering network, consensus, view changes, recovery, GC. Named examples:
- `surrealdb_ds_consensus_fast_quorum_timeouts_total`
- `surrealdb_ds_finalize_prepare_retries_total`
- `surrealdb_ds_recovery_outcome_drain*`
- `surrealdb.ds.epoch_fence_drops` (labelled by `direction`)

**Implied shape:** two-phase finalize with prepare retries; membership epochs with directional fencing; view changes; bounded outcome-journal catch-up; Raft-flavoured multi-group consensus.

**Env vars (public, 8 rows):**

| Variable | Default | Tune when |
|---|---|---|
| `SURREAL_DS_NTW_INBOUND_BYTES` | `3 * MAX_MSG_BYTES` (768 MiB) per `MessageClass` | `QUIC inbound bytes ... saturation` log |
| `SURREAL_DS_NTW_INFLIGHT_PROCESSING_CAP` | `max(384, num_cpus*32)` | `QUIC inbound handlers ... saturation` log |
| `SURREAL_DS_MAX_READ_OPERATIONS` | 192 | steady-state RSS pressure / scan-heavy |
| `SURREAL_DS_MAX_WRITE_OPERATIONS` | 96 | `Write operations limit saturation` log |
| `SURREAL_DS_CONSENSUS_FAST_QUORUM_TIMEOUT_MS` | impl default | `..._fast_quorum_timeouts_total` rising |
| `SURREAL_DS_RETRY_BASE_MS` / `_MAX_MS` | 500 / 5000 | `..._finalize_prepare_retries_total` rising |
| `SURREAL_DS_ROCKSDB_BLOCK_CACHE_SIZE` (3.1.1+) | `max(memory/2 - 1GiB, 16MiB)` | RSS pressure. Largest single memory consumer; full-text index reads served entirely through it; vector index reads hit it on load + cache misses |
| `SURREAL_DS_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` (3.1.1+) | memory-derived 2–32 | RSS during sustained writes. Memtable ceiling = this × `WRITE_BUFFER_SIZE` × 4 heavy column families; cap this first when headroom is tight |

**Env vars known only from release notes:**
- `SURREAL_DS_RECOVERY_OUTCOME_JOURNAL_ENTRIES` — default 262,144, `0` disables
- `SURREAL_DS_TRANSACTION_WRITE_SET_LIMIT_OPS` — default 100,000, `0` disables; sits beside a 32 MiB byte cap; over-bound fails fast at coordinator with `TransactionTooManyOperations`; warning at half bound; **keep uniform across the fleet**
- `SURREAL_DS_STARTUP_NORMAL_TIMEOUT` — unset it if you set it to `0` on 3.3.0-beta.1; `=0` disables both Enterprise startup gates
- `SURREAL_STARTUP_OPERATION_TIMEOUT` — docs advise keeping `180`; default `60` is "tight for a consensus engine on a cold cluster"

### 5.4 Real failure modes (from 3.3.0 release notes, SurrealDS section)
These describe the actual implementation more honestly than any marketing page:
- Lost-write race: two concurrent read-modify-writes of one key both committing; could let a `UNIQUE` index hold a duplicate.
- Read on the RocksDB store committing after missing a concurrent write to a key a later range delete covered.
- Recovery aborting an already-committed transaction after a replica learned of the commit via catch-up replay.
- Backup coordinator deciding a transaction differently from its original coordinator.
- Replicas holding **contradictory outcomes** for one transaction refusing to recover or install a view. Now converge and log both.
- Transaction in flight across a membership change stalling reads on that node for several minutes. Now rejoins new membership or fails with retryable conflict.
- Pending view change wedging startup warm-up → node exits and restarts into the same state.
- Failed statement leaving behind records and index entries written by a synchronous event.
- **New `CommitOutcomeUnknown`** error: tells the client to read back rather than replay a write that may have landed.
- QUIC stack moved to `quinn-proto` 0.11.18 in 3.3.0 to fix **three remote memory-exhaustion issues and a remote panic** in the inter-node transport.

### 5.5 Performance signals published
3.3.0-beta.1 release notes, SurrealDS cold starts:
- Cold-start convergence tails: **4.44s → 0.02s (n=3)**, **14.75s → 4.61s (n=5)**, default-config worst case **86.9s → 13.7s**. Achieved via view-change retransmission, formation-aware backoff, concurrent outcome-donor drains.
- The Phase-1 outcome drain used to re-page a donor's **entire outcome history on every recovery entry — O(all transactions ever committed)**. Fixed with a bounded in-memory write-order journal + delta requests, falling back to full stream.

---

## 6. Security posture

`v3.3.0` shipped a large batch of security fixes from SurrealDB's internal review + external reviewers, affecting `v3.2.4` and earlier. Highlights:
- **Critical** — cross-tenant access via `USE` / `Surreal-NS` / `Surreal-DB` headers (GHSA-vx2p-7hhm-wv62, GHSA-2v9j-645m-pcrq, GHSA-w5v9-jc5f-5rmc)
- **High** — field-level `SELECT` permission bypass through `PATCH` pointers; same through `LIVE SELECT` `$after`/`$before`; MCP session takeover between users of one JWT/bearer access method; SSRF via IPv6 transition addresses
- **Moderate/Low** — cross-tenant record read via `FETCH`, GraphQL schema disclosure, computed fields disclosing restricted values, indexes on restricted fields acting as a value oracle, unauthenticated `revoke` invalidating refresh tokens, deeply nested values crashing the server, Surrealism module output exhausting host memory, non-UTF-8 fetch headers crashing the server, port-specific `--deny-net` bypass via host aliasing
- Behaviour changes in 3.3.0: values nested >256 deep rejected on write; cross-tenant requests now `NotAllowed` (403); `@@` against an unreadable field refused; `revoke` requires the refresh token itself; `AUTHENTICATE` clauses run with the access method's own privileges instead of root; `COMPUTED` runs with its definer's rights; record users confined to their own NS/DB on every write.

Also note: 3.3.0 release binaries embed CJK dictionaries for the `SEGMENT` tokenizer → **+~150 MB binary**.

---

## 7. Upgrade / migration risk register

- **2.x → 3.x**: no in-place path. Use a v3-compatible export from the 2.x data, import into 3.x. `surreal fix` only converts 1.x → 2.x layout and only 2.x binaries implement it.
- **3.2 → 3.3.0**: datastore upgrades in place, automatic data migrations on first start. **But nodes do not fully interoperate during rollout** — a sequence, schema change or new table created on an upgraded node is invisible to/unreadable by 3.2 nodes, and a 3.2 node fails kNN against an HNSW/DiskANN index written by 3.3. Let index builds finish before starting the rollout.
- **Rollback past 3.2 requires restoring a pre-upgrade export.** A 3.2 binary starts on a 3.3-opened datastore but reads migrated data wrongly; a pre-3.3.0 build records no migrations and starts on a migrated datastore and reads it wrongly. A build lacking a recorded migration now **refuses to start** and names the migration.
- **Migration ledger**: recorded cluster-wide, each migration runs once regardless of node count, interrupted runs resume, a failed migration aborts startup. Datastores created on 3.3.0+ record the full historical set as applied at creation.
- 3.0.0–3.2 bug (fixed by the 3.3.0 migration): keyspace collision where `DEFINE SEQUENCE` keys sorted inside the table-name band, so any table whose name began with `sq` broke `INFO FOR DB`, `REMOVE DATABASE` (leaving the DB undroppable), and exports containing sequences. No sequence needed to exist to hit this.
- Other 3.3.0 semantic changes: `UPDATE`/`UPSERT` evaluate `WHERE` first and read one pre-write image; field `DEFAULT`/`VALUE`/`COMPUTED` clauses now evaluate in dependency order (renaming a field can no longer break a schema); `ASSERT` runs once all fields hold final values; rolled-back transactions count as `outcome="cancelled"`; `INFO FOR INDEX` always includes `compacting`; `SURREAL_FTS_DOC_IDS_BATCH_SIZE` renamed to `SURREAL_TABLE_DOC_IDS_BATCH_SIZE`; ISO GQL (Cypher) **on by default** without `--allow-experimental`; storage thread pool no longer pins workers to cores.
- `mem://` loses `VERSION` support in 3.3.0.

---

## 8. Other 3.x capabilities relevant to scaling

- **gRPC transport** (`grpc://` / `grpcs://`) + `query_stream` WebSocket RPC method; `surreal sql` connects over gRPC. `SURREAL_GRPC_MAX_MESSAGE_SIZE` (defaults to `SURREAL_HTTP_MAX_RPC_BODY_SIZE`); raise it for bulk embedding ingest.
- **End-to-end streaming results** — rows arrive as produced, not buffered.
- **Postgres wire protocol** — `--postgres-bind`, TLS + SCRAM-SHA-256, `DEFINE USER` stores a SCRAM verifier. Any Postgres client/driver (psql, JDBC, npgsql) can run SurrealQL or ISO GQL.
- **`--durable-sessions`** — persists client-attached HTTP RPC sessions in the datastore with sliding idle TTL (default 24h) so they survive restarts and can resume on any node sharing the storage. Stored **unencrypted**; sticky routing recommended; concurrent multi-node use is best-effort.
- Health/readiness split: HTTP listener binds before datastore startup transactions run, so `/health` answers immediately and query endpoints 503 with `Retry-After` while `/ready` reports not-ready.
- `--query-timeout` now enforced at transport, returns 504.
- `/signin` + `/signup` optional per-address rate limiting → 429 + `Retry-After`.
- `SELECT ... FOR UPDATE` (commit-time locked reads) on every community backend.
- Bitmap index fusion, pre-filtered vector search, `EXPLAIN ANALYZE` shows plan + filter tier.
- Graph: `LIGHTWEIGHT` relations (no stored edge payload), `INLINE` fields, `INLINE EDGES`/`INLINE REFERENCES` caches (0–256) → filtered traversals answered from adjacency without reading edge records.
- `DEFINE ACCESS ... CONTEXT` freezes a payload once per session into `$session.data`, so a row permission no longer runs a subquery per row. `DEFINE ACCESS ... AUDIENCE` enforces JWT `aud`.
- Full-text indexing 2.3–2.6× faster; new `SEGMENT` tokenizer for CJK.
- Mature embedded JS engines rebuilt with the server as `@surrealdb/node-native` and `@surrealdb/wasm-native`.

---

## 9. Hypotheses / inferences (my assessment, not SurrealDB's claims)

Labeled with confidence. These are the things I believe from triangulating docs, release notes, and metric/env-var names.

**H1 — SurrealDS is Raft-derived multi-group consensus with epoch fencing, not a novel protocol.** Confidence: high. Evidence: `consensus_fast_quorum_*`, `finalize_prepare_retries_total`, `epoch_fence_drops` with a `direction` label, "view changes", membership epochs, "no elected leader" + per-AZ write nodes. Fast-quorum path with a slower fallback path matches multi-group Raft. *Test:* Enterprise runbook, or the 3.1/3.2 release-note sets which likely narrate the initial design.

**H2 — Storage is RocksDB per node plus a shared object-storage durable tier.** Confidence: high. Evidence: `SURREAL_DS_ROCKSDB_BLOCK_CACHE_SIZE` / `_MAX_WRITE_BUFFER_NUMBER` / `_WRITE_BUFFER_SIZE` described as applying "when a node's store path selects the RocksDB durable backend", "four heavy column families", "full-text index reads are served through it entirely". So: local RocksDB for hot data (block cache, memtables), object storage for durability/DR. This is not a sharded LSM; it's log-structured on object storage with a local cache. *Test:* ask sales for the store-path syntax, or `strings` the Enterprise binary.

**H3 — QUIC transport is class-partitioned with explicit backpressure budgets.** Confidence: medium-high. Evidence: inbound byte budget is "per `MessageClass`", separate handler concurrency cap, and both emit their own saturation warning logs. *Test:* run a cluster and provoke the saturation warnings to see the class names.

**H4 — Inter-node traffic is a real, live security surface.** Confidence: high. Evidence: 3.3.0 had to patch three remote memory-exhaustion CVEs and a remote panic in `quinn-proto` specifically in the SurrealDS QUIC transport. A distributed engine's mesh transport should be treated as internet-exposed until proven otherwise.

**H5 — The engine was still maturing at 3.3.0.** Confidence: high. Evidence: convergence worst case was 86.9s before the fix; the outcome drain was O(all transactions ever committed) before 3.3.0; 3.3.0's SurrealDS section is dominated by lost-write races, contradictory replica outcomes, and transactions aborted after committing. This is a young distributed engine, not a decade-hardened one.

**H6 — `CommitOutcomeUnknown` is the API contract to design around.** Confidence: high. It is the SDK-level signal that a write may or may not have landed, and the instruction is *read back, don't replay*. Any application-layer retry logic on a SurrealDS cluster must handle this distinctly from definite failure. This is the single most important integration detail for correctness.

**H7 — Horizontal scaling is a licensing decision, not an architecture decision, for most teams.** Confidence: high. Community has no supported distributed storage. The engineering cost of a self-hosted SurrealDS cluster (k8s operator, licence key with an external online dependency, gated tuning reference, gated runbooks) exceeds the $420/mo floor for Cloud Scale unless you have hard data-residency, air-gap, or compliance requirements.

**H8 — Air-gapped operation is possible but fragile.** Confidence: medium-high. Requires `SURREAL_LICENSE_STATE_PATH` on a persistent volume, and you get a **24-hour** budget from the last signed response — a single budget that restarts don't reset. Practically that means either a recurring outbound link to keygen.sh or accepting a periodic forced outage. Enterprise sales does advertise on-prem / sovereign / air-gapped, so there is presumably a negotiated answer; it just isn't in the public docs.

**H9 — `mem://` is now a dev/test engine, not a production candidate.** Confidence: medium-high. It lost `VERSION` support in 3.3.0. Docs already position RocksDB as the conservative production pick and SurrealKV as beta. `mem://` has no documented persistence story worth betting on beyond snapshots.

**H10 — Expect Community → Enterprise storage-backend friction to be low, but operational friction to be high.** Confidence: medium. Same binary family, same schema, same SDK, licence key is the only stated difference. The friction is entirely in running consensus: node count discipline (odd, ≥3), memory sizing for `SURREAL_DS_ROCKSDB_BLOCK_CACHE_SIZE` on memory-constrained pods, uniform `SURREAL_DS_TRANSACTION_WRITE_SET_LIMIT_OPS` across the fleet, and `SURREAL_STARTUP_OPERATION_TIMEOUT=180`.

**H11 — Multi-region is aspirational for self-hosted.** Confidence: medium. Object-storage-backed cross-region replication is listed as *near-term roadmap* on the Scale launch blog and "coming soon" on the pricing page. The 11-nines durability story implies the mechanism exists, but nothing is GA. Do not plan around it.

**H12 — For most self-hosted workloads the honest ceiling is "one very large RocksDB node, vertically scaled, with good backups."** Confidence: high. Community Edition supports exactly that, the 3.0 benchmark work (34.8k OP/s create on SurrealKV, ~2,600% over 2.x) is all single-node, and 3.3.0's index/planner work benefits single-node far more than a cluster you're not allowed to run.

---

## 9b. Code archaeology — is SurrealDS its own crate?

**Conclusion: yes, almost certainly a separate crate in a private workspace, consuming the public crates. Confidence: high.** All evidence below is from the public repo on `main` at `3.3.0-nightly`.

### 9b.1 Public workspace has no DS crate
Root `Cargo.toml` members: `.`, `surrealdb`, `surrealdb/{ast,collections,common,core,mcp,parser,server,strand,token,types}`, `surrealdb/types/derive`, `surrealml/core`, `surrealism{,/macros,/runtime,/types,/demo}`, `profiling`. No SurrealDS member.
`core/src/kvs/` subdirs: `rocksdb/`, `surrealkv/`, `tikv/`, `mem/`, `indxdb/`, `cache/`, `version/` — one dir per engine, **no `surrealds/`**.

### 9b.2 `surrealdb-core` exports an out-of-tree extension seam
`surrealdb/core/src/kvs/ds.rs:482`:
```rust
pub trait TransactionBuilderFactory: TransactionBuilderFactoryRequirements {
    /// Immutable state threaded into router construction after datastore startup.
    type RouterState: Clone + Send + Sync + 'static;
    fn new_transaction_builder(&self, path: &str, canceller: CancellationToken, config: ConfigMap)
        -> impl Future<Output = Result<TransactionBuilderParts<Self::RouterState>>> + Send;
```
`DatastoreBuilder::build_with_factory_path<F>(path, composer)` at `core/src/kvs/ds/builder.rs:199`, where
```rust
pub async fn build_with_path(self, path: &str) -> Result<Datastore> {
    self.build_with_factory_path(path, CommunityComposer()).await
}
```
→ the Community path is simply *the default composer*.

### 9b.3 The whole server boot is generic over the composer
`surrealdb/server/src/cli/start.rs:213`: `C: TransactionBuilderFactory + RouterFactory`, threaded through `build_observability::<C>`, `dbs::init::<C>`, and router construction. Additional composer hooks observed: `BucketStoreProvider`, `ObservabilityProvider` (`create_observer_with_runtime`, `audit_counters`, `slow_query_counters`), `check_config`, `live_query_broker`, `datastore_node_id`, `http_endpoint`, `endpoint_resolver`.

### 9b.4 Public comments name the absent implementation
Verbatim from `core/src/kvs/ds/builder.rs`:
- "Pull the local node's public HTTP endpoint from the composer (**clustered editions** surface it from their topology config). Persisted on every `Node` row this datastore writes so other cluster members can discover it via the catalog."
- "Resolve the broker once: explicit `with_live_query_broker` wins, otherwise let the composer decide (**community returns a `LocalMessageBroker`, enterprise returns its relay broker**)."
- "WASM doesn't run **clustered brokers** and its transaction types aren't `Send + Sync`".
- `router_state` "is immutable and must be passed to the matching router factory during server startup".

### 9b.5 Duplicated, not shared, config namespaces
| Community (in `surrealdb_core::cnf`) | SurrealDS |
|---|---|
| `SURREAL_ROCKSDB_BLOCK_CACHE_SIZE` | `SURREAL_DS_ROCKSDB_BLOCK_CACHE_SIZE` |
| `SURREAL_ROCKSDB_JOBS_COUNT` | `SURREAL_DS_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` |
| `SURREAL_ROCKSDB_GROUPED_COMMIT*` | `SURREAL_DS_ROCKSDB_WRITE_BUFFER_SIZE` |
| ~30 knobs | **3 knobs** |

Same underlying RocksDB, independently prefixed and independently tuned → its own config module, not a reuse of core's. The 3-vs-~30 gap suggests a deliberately curated surface (either good hygiene, or consensus internals kept unconfigurable — undeterminable from outside).

### 9b.6 They had to split it
The Community binary must not link `quinn`/QUIC or consensus deps. Corroborated by 3.3.0 tagging the quinn-proto security fixes as `[Enterprise]`.

### 9b.7 A private workspace is trivially feasible
`surrealdb-core`, `surrealdb-server`, `surrealdb-types` are published on crates.io at `3.3.0`. Workspace is `resolver = "3"`, `edition = "2024"`, license Apache-2.0. A private workspace can build the Enterprise binary against them as ordinary versioned deps.

### 9b.8 Probable shape
```
private workspace (unpublished)
├── surrealdb-enterprise        → binary; links keygen.sh validation + quinn
├── surrealdb-ds (or -cluster)  → consensus, QUIC transport, backup coordinator, GC, recovery
│     impl TransactionBuilderFactory { type RouterState = DsRouterState }
│     impl BucketStoreProvider / RouterFactory / ObservabilityProvider
└── deps: surrealdb-core, surrealdb-server, surrealdb-types (crates.io)
```

**Alternative not ruled out:** a private branch/fork adding in-tree workspace members. But that would make the public composer seam redundant — you don't build a public trait abstraction for code in the same crate. The seam only pays for itself if the implementor is out-of-tree.

**Method note:** verified via `gh api repos/surrealdb/surrealdb/contents/...` on `main`, plus `gh search code --repo surrealdb/surrealdb`. No Enterprise source, image, or binary was inspected.

**H13 — SurrealDS is a separate crate in a private workspace, and its source has never been public.** Confidence: high. See section 9b. The strongest single tell is the deliberate `TransactionBuilderFactory` / `RouterFactory` composer seam with an associated `RouterState` type, plus comments in the public source that explicitly reference "clustered editions" and "enterprise returns its relay broker". *Test:* strings/symbol table of `surrealdb/surrealdb-enterprise:latest` would likely reveal crate paths (e.g. `surrealdb_ds::…`) in panic messages and backtraces — a cheap, no-licence-required probe. Panic/backtrace metadata in the public image is a plausible leak vector.

**H14 — The `SURREAL_DS_*` surface being 3 RocksDB knobs vs community's ~30 is deliberate curation, not incompleteness.** Confidence: medium. Rationale: an immature engine exposing 30 knobs would be a footgun; the release notes do show them adding bounded limits (`SURREAL_DS_TRANSACTION_WRITE_SET_LIMIT_OPS`, recovery journal entries) rather than opening up tuning. *Test:* the Enterprise deployment guide, or asking support why the DS surface is smaller.

---

## 10. Open questions worth asking SurrealDB sales / Enterprise support

1. What is the datastore URL scheme / `surreal start` syntax for SurrealDS? (Not in the public CLI reference.)
2. Full `SURREAL_DS_*` reference + the complete `MessageClass` list.
3. Object storage: is it required, or is local-disk-only a supported configuration? Which providers are certified?
4. Is the Kubernetes operator open source, or delivered only under contract? Does it work on non-managed k8s (KIND, k3s, bare metal)?
5. What is the supported rolling-upgrade policy across 3.2/3.3 for a SurrealDS cluster given the stated interoperability limits?
6. Minimum viable node specs (vCPU / RAM / disk) for a 3-node production cluster, and how block-cache sizing scales with dataset size.
7. What does `SURREAL_LICENSE_STATE_PATH` do in a fully air-gapped install — is there an offline licence variant, and what is the real-world renewal cadence?
8. Does `CommitOutcomeUnknown` require SDK retry-helper support in each language SDK? Which ones ship it?
9. Any published benchmarks for the distributed engine (throughput, p99 commit latency at 3/5/7 nodes)?
10. Is SurrealDS single-tenant-per-cluster only, or does it support tenant/compute isolation comparable to Cloud Scale?
11. Is the `surrealdb-enterprise` source available under contract (source escrow, or a private repo with build access)? The public crates expose a `TransactionBuilderFactory` / `RouterFactory` composer seam, which implies the DS layer is out-of-tree — is that source ever shared?

## 11b. Things you can learn without a licence
- `docker pull surrealdb/surrealdb-enterprise:latest` and inspect symbols/panic metadata/backtrace strings — crate paths in Rust panic messages often leak the crate structure. (No licence needed to pull; a key is needed to *start*.)
- Community `surrealdb-core` is on crates.io at 3.3.0, so the extension seam's contract can be read directly from docs.rs.
- Enterprise release-note sections (`SurrealDS (Enterprise)` headings in each release) are the highest-signal public source on internals.

---

## 11. Recommendation for the apothic monorepo context

Given a shared SurrealDB on Fly.io with `.dev.vars`-style credentials per app:

- **Stay single-node** and scale vertically. It is the only fully supported self-hosted path, and 3.3.0's index/planner/query-engine work lands entirely on that path.
- If HA is needed: **Cloud Scale** ($420/mo floor) beats building a SurrealDS cluster, both on cost and on the fact that the operator and runbooks are contract-gated.
- Self-hosted Enterprise only if data residency, air-gap, or FIPS is a hard requirement — and then confirm the licence/air-gap story explicitly, since the public docs give a 24-hour online-validation budget.
- **Upgrade to 3.3.0** if on ≤3.2.4 (Critical cross-tenant advisory). Take an export first; 3.2→3.3 rollback is export-only.
- Keep `tikv://` out of anything resembling production.

---

## 12. Sources

Docs
- Deployment models — https://surrealdb.com/docs/manage/self-hosted/deployment-models
- Multi-node — https://surrealdb.com/docs/running/multi-node
- Self-hosted overview — https://surrealdb.com/docs/manage/self-hosted
- Kubernetes (single-node RocksDB) — https://surrealdb.com/docs/manage/self-hosted/kubernetes
- Managed Kubernetes (EKS/GKE/AKS) — https://surrealdb.com/docs/manage/self-hosted/managed-kubernetes
- Backups & recovery — https://surrealdb.com/docs/manage/self-hosted/backups-and-recovery
- Upgrades & patching — https://surrealdb.com/docs/manage/self-hosted/upgrades-and-patching
- Enterprise Edition (database) — https://surrealdb.com/docs/manage/enterprise/product/database-enterprise
- Enterprise observability — https://surrealdb.com/docs/manage/observability/enterprise-observability
- Observability / metrics — https://surrealdb.com/docs/manage/observability/observability
- Environment variables — https://surrealdb.com/docs/reference/cli/surrealdb-cli/environment-variables
- `surreal start` — https://surrealdb.com/docs/reference/cli/surrealdb-cli/commands/start
- Docs index (`llms.txt`) — https://surrealdb.com/docs/llms.txt
- Full docs corpus (`llms-full.txt`) — https://surrealdb.com/docs/llms-full.txt

Marketing / blog / releases
- SurrealDS platform page — https://surrealdb.com/platform/surrealds
- Technical deep dive — https://surrealdb.com/surrealdb/deep-dive
- Introducing SurrealDB Cloud Scale (2026-07-02) — https://surrealdb.com/blog/introducing-scale-surrealdb-cloud-built-for-high-availability-and-scale
- Introducing SurrealDB 3.0 — https://surrealdb.com/blog/introducing-surrealdb-3-0--the-future-of-ai-agent-memory
- SurrealDB 3.0 benchmarks (single-node) — https://surrealdb.com/blog/surrealdb-3-0-benchmarks-a-new-foundation-for-performance
- Release 3.3 notes — https://surrealdb.com/releases/3.3
- Release 3.0 notes — https://surrealdb.com/releases/3.0
- v3.0.0 GitHub release — https://github.com/surrealdb/surrealdb/releases/tag/v3.0.0
- Enterprise edition page — https://surrealdb.com/enterprise
- Pricing — https://surrealdb.com/pricing
- Helm charts — https://github.com/surrealdb/helm-charts
- GKE guide (TiKV deprecation note) — https://surrealdb.com/docs/build/deployment/self-hosted/google-gke

Also downloaded for local analysis: `/tmp/surreal_llms.txt` (docs index), `/tmp/surreal_full.txt` (full docs corpus, 7.1 MB).