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
| `PUBAPI` | Used. `surrealdb-kvs` / `surrealdb-kvs-any` 3.3.0. BUSL-1.1, see ADR-0002. |
| `SRC` | Read for interface shape during archaeology. **No code copied, translated, or paraphrased into our source.** Findings recorded as observations in DECISIONS.md. |
| `BIN` | **Never used.** Declared off-limits. |

**Firewall.** `upstream/surrealdb/` — a pinned checkout used for reading
interfaces and as a black-box oracle — is kept textually and operationally
separate from the crates we build. It is **never a build input**. The only
SurrealDB code in the build graph is the three published crates we deliberately
link, and that is a recorded decision (ADR-0002), not an accident.

**One correction worth recording.** Our first architecture pass read the upstream
default branch and drew twelve conclusions from it. Checking crates.io showed
that branch predates the v3.3.0 release by twenty days and has a different
layout, so most of those conclusions were wrong — including the shape of the
extension seam and whether the storage contract was even public. Ten of twelve
API assumptions in our first implementation were also wrong. Treat every
branch-derived claim as a hypothesis until a published artifact confirms it
(ADR-0001, ADR-0004).

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
| R-0030 | `v3.3.0`'s `TransactionBuilder` has no `extension()` hook; the pre-release tree had one | PUBAPI | `surrealdb-kvs` 3.3.0 `builder.rs` | 2026-10-01 | recorded |

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