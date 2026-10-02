# Progress log

Dated, append-only. Newest at the top. An entry records what was *actually
verified*, not what was intended.

Status legend: `[x]` done and verified · `[~]` in progress · `[ ]` not started ·
`[!]` blocked

---

## 2026-10-01 — Project initialised

### Research and reconnaissance

- [x] Surveyed SurrealDB self-hosting and horizontal scaling at v3.3.0. Findings
      in `docs/research/SURREALDS-RESEARCH.md`.
- [x] Established the headline: Community Edition is single-node only;
      horizontal scaling requires the SurrealDS distributed storage engine,
      which self-hosted means a closed Enterprise binary on a contact-sales
      licence.
- [x] Measured SurrealDS's public documentation surface: **no reference page
      exists**; the full 7.1 MB docs corpus mentions it 15 times; the complete
      `SURREAL_DS_*` reference is contract-gated.
- [x] Extracted what public information does exist: ~30 `surrealdb.ds.*`
      metric names, 8 documented tuning variables, and the architecture claims
      (leaderless quorum consensus, per-AZ write nodes, object-storage durable
      tier, log-delta recovery).
- [x] Recorded 14 labelled hypotheses (H1–H14) with confidence levels and
      falsification tests.

### Repository archaeology

- [!] **First analysis was aimed at the wrong tree.** The repository's default
      branch `origin/main` (`18971ff`, 2026-09-04) is twenty days *older* than
      the `v3.3.0` tag (`238bfeb`, 2026-09-24) and is on a divergent lineage.
      Our initial conclusions about the extension seam were derived from
      pre-release code and **have been corrected** — see ADR-0001 and ADR-0004.
- [x] Settled it against published artifacts: `surrealdb-kvs`,
      `surrealdb-kvs-any`, `surrealdb-kvs-rocksdb`, `surrealdb-engine-api`,
      `surrealdb-datastore`, `surrealdb-catalog` are all published at `3.3.0`
      and exist only in the modular layout. The tag is authoritative.
- [x] Cloned and pinned `v3.3.0` to `/tmp/forktest/sdb` for reference.
- [x] Mapped the real extension seam:
      `surrealdb-kvs-any::BackendProvider` — `schemes()`, `accepts_bare()`,
      `connect(ConnectContext)`. Registered via `Backends::register`. The
      upstream doc comments name "the enterprise distributed store" as the
      intended external implementor, and `registry.rs` carries a test
      `ExternalProvider` that mimics exactly this shape.
- [x] Confirmed the storage contract is fully public in `surrealdb-kvs`:
      `Transactable`, `TransactionBuilder`, `Metric`, `Metrics`, and the
      `key` / `value` encoding contract modules are all `pub`.
- [x] Found `surrealdb-kvs-test`, an in-tree shared conformance suite for KV
      backends with `harness = false` registration of one `TestBackend` per
      engine. Not published (`publish = false`). This is our acceptance
      criteria.

### Licensing — the finding that changes the plan

- [x] **SurrealDB is BUSL-1.1, not Apache-2.0.** The repository `LICENSE` is
      Business Source License 1.1. Additional Use Grant permits use but
      **prohibits use as a "Database Service"**. Change Date **2030-01-01**,
      then Apache-2.0.
- [x] Verified against crates.io: every SurrealDB crate carries
      `license = "non-standard"`, i.e. BUSL-1.1. The Apache-2.0 badge in
      SurrealDB's own `CARGO.md` is inaccurate.
- [x] Analysed the consequence: BUSL-1.1 has no non-compete clause, so
      *independent development is permitted*; but any crate that **links** a
      SurrealDB crate becomes a derivative work under BUSL. That splits the
      project into two viable shapes with very different economics.
- [!] **Open owner decision (ADR-0002).** Path A (link the crates, get the
      front end and conformance suite free, accept BUSL on the derived crates)
      versus Path B (zero SurrealDB dependencies, Apache-2.0 end to end,
      substantially more work). Scaffold currently defaults to Path A, which
      is reversible at the module boundary.

### Scaffolding

- [x] Repository structure, `LICENSE` (Apache-2.0), `NOTICE` stating the
      licensing boundary explicitly.
- [x] `README.md`, `PLAN.md`, `DECISIONS.md`, `PROVENANCE.md`, this log.
- [x] `Makefile`, `scripts/bootstrap.sh`, `scripts/audit-upstream.sh`.

### Phase 0 — the seam works

**The largest unknown is retired: a third-party crate can register a SurrealDB
storage backend and have it selected by URL scheme, alongside the first-party
engines.**

- [x] `crates/surrealdb-ds` type-checks against the published crates
      (`surrealdb-kvs`, `surrealdb-kvs-any`, `surrealdb-cnf` 3.3.0).
      **0 errors, 0 warnings.**
- [x] `BackendProvider` implemented, claiming schemes `ds` and `ds+mem`.
- [x] `TransactionBuilder` implemented: `name`, `new_transaction`,
      `shutdown`, `register_metrics`, `collect_u64_metric`.
- [x] `Transactable` fully implemented — all 36 methods at the v3.3.0 surface,
      including zero-copy `ScanCursorKeys` / `ScanCursorVals` with
      `KeysBatch::from_parts` / `ValsBatch::from_parts`.
- [x] `crates/surrealdb-ds-server` type-checks. **0 errors, 0 warnings.**
- [x] Runtime verification. `make run` constructs through our provider:
      ```
      INFO surrealdb::core::kvs::ds: Starting kvs store in ds+mem
      INFO surrealdb_ds::provider: constructing engine scheme="ds+mem"
      INFO surrealdb_ds_server: engine ready: surrealds
      ok: constructed backend for ds+mem://
      ```
- [x] Coexistence check — the same binary also constructs `rocksdb:/tmp/…`,
      `memory` and `ds://node1`, so our provider does not shadow or get shadowed
      by any first-party engine. Unknown scheme `bogus://x` fails cleanly with
      "Unable to load the specified datastore".
- [x] `scripts/audit-upstream.sh` run: confirms all 13 published SurrealDB crates
      are `non-standard` (BUSL-1.1) at 3.3.0, and that `surrealdb-kvs-test` and
      `surrealdb-node` are **absent** from crates.io, as expected.

#### API corrections made while getting it to compile

Our first attempt was written against the pre-release `main` trait and did not
compile. The v3.3.0 surface differs in ways worth recording:

| Pre-release assumption | Actual v3.3.0 |
| --- | --- |
| `new_transaction(write: bool, lock: bool)` | `new_transaction(TransactionType)` — `Read`/`Write` enum |
| no `name` method | `TransactionBuilder::name() -> &'static str` is required |
| `ScanCursorKeys` in `cursor` module | in `surrealdb_kvs::api`, and `Send + Sync` |
| `Batch<Key>` | `Batch<Vec<u8>>`; `Batch::new(next, result)` |
| `KeysResult { keys }` | also `key_bytes: u64` |
| `ScanResult { values: Vec<Val> }` | `values: Vec<(Vec<u8>, Val)>` + `key_bytes` + `value_bytes` |
| `GetMultiResult { values, found, bytes }` | `values: Vec<Option<Val>>`, `records`, `value_bytes` |
| `delc`/`clrc` take `Option<Val>` | take `Option<&'a [u8]>` |
| `Error::ConditionNotMet` | `Error::TransactionConditionNotMet` |
| ranges are `std::ops::Range<Key>` | `KeyRange<'a> { start: Key<'a>, end: Key<'a> }` |
| `BoxFut` at crate root | `surrealdb_kvs::api::BoxFut` |
| `BuildError` from `construct` | `Backends::new_transaction_builder(path, CancellationToken, ConfigMap)` returns `Box<dyn TransactionBuilder>` directly |

**Lesson worth keeping (ADR-0004):** every one of these would have been a silent
mistake if we had trusted the branch we happened to read first. Read the
published crate, not the branch.

#### Also learned

- `TransactionBuilder` at v3.3.0 has **no `extension()` hook** — the pre-release
  tree had one for TiKV-specific operations. Phase 4's bucket/object-store work
  goes through `BucketStoreProvider` in `surrealdb-core` instead.
- `Backends::new_transaction_builder` does not return `TransactionBuilderParts`;
  it returns the boxed builder directly, so there is no router-state threading at
  the registry layer in v3.3.0.

### Honest state

The extension point is real, open, and works from an external crate. That was the
single biggest risk in the project and it is now retired.

What is **not** done, and should not be assumed:

- Storage is `BTreeMap`-backed and **in-memory**. No durability, no persistence,
  no concurrency control.
- Writes are **not** staged. `set` writes straight through and `cancel` rolls
  nothing back. That is not the contract.
- `getu` (locked read) returns `UnsupportedLockedReads`, so `SELECT … FOR UPDATE`
  has no conflict guarantee yet.
- `safe_timestamp` is the single-node default. **Unsafe on more than one node.**
- `rollback_to_save_point` is a no-op.
- `compact` declines rather than lying about having done work.
- The upstream conformance suite has **not** been run — it needs the vendored
  tree (open question 2).
- The HTTP surface is not wired up; the binary constructs an engine and exits.

Next action: vendor `surrealdb-kvs-test` and get our engine through the
conformance suite. That is the real Phase 0 exit criterion in PLAN.md.

---

## Open questions carried forward

1. **ADR-0002 licensing decision — Path A or Path B. Owner call, and now the
   only thing between the project and a clean bill of health.**
2. Can `surrealdb-kvs-test` be vendored on its own, or does it drag in most of
   the workspace? (It depends only on `inventory`, `surrealdb-kvs`,
   `libtest-mimic` and `tokio` — likely cheap, but unverified.)
3. Does `surrealdb-server` expose a public init path that accepts a
   caller-constructed `Backends` registry, or does the CLI build its own?
4. Does v3.3.0 still carry the `safe_timestamp` contract? It existed in the
   pre-release tree with an explicit warning naming SurrealDS. If present, the
   distributed engine must honour it; if absent, the requirement moves to our own
   correctness work.
5. What is the exact precondition semantics for `putc`/`delc` when `chk` is
   `None` and the key is absent versus present? Our implementation collapses
   these cases and the suite may disagree.