# Progress log

Dated, append-only. Newest at the top. An entry records what was *actually
verified*, not what was intended.

Status legend: `[x]` done and verified · `[~]` in progress · `[ ]` not started ·
`[!]` blocked

---

## 2026-10-02 — Phase 0 conformance: the engine passes the upstream suite

**Phase 0's remaining exit criterion is met.** `make test` runs upstream's own
backend contract suite against our engine, through the public provider seam, and
it is green.

```
$ make test
test result: ok. 10 passed;  0 failed;   0 ignored   (surrealdb-ds unit)
test result: ok. 77 passed;  0 failed;  12 ignored   (surrealds::… conformance)
test result: ok.  6 passed;  0 failed;   0 ignored   (surrealdb-ds serializable)
test result: ok.  1 passed;  0 failed;   0 ignored   (surrealdb-ds doctest)
test result: ok.  0 passed;  0 failed;   3 ignored   (vendored suite doctests)
```

77 of 77 runnable tests pass. The 12 ignored are the suite's own `only`/`except`
filters, not skips we added — see "What the suite does not check for us" below.

### Vendoring

- [x] `surrealdb-kvs-test` is vendored at `vendor/surrealdb-kvs-test/`: upstream's
      `surrealdb/kvs-test` at tag `v3.3.0` (`238bfeb`), `src/` byte-for-byte,
      BUSL-1.1 `LICENSE` alongside it. Only `Cargo.toml` is ours, and only because
      upstream inherits its metadata and lint table from a workspace root that
      does not exist here.
- [x] **Open question 2 answered: it is cheap.** It depends only on `inventory`,
      `surrealdb-kvs`, `libtest-mimic` and `tokio`. Nothing else in the workspace
      comes with it.
- [x] Verified the published `surrealdb-kvs` 3.3.0 source is byte-identical to the
      tree at the tag (`api.rs`, `builder.rs`, `err.rs`, `timestamp.rs` all
      `diff`-clean), so the suite and the crate we link are the same release.
- [x] `NOTICE` now states the vendoring as a third licensing path distinct from
      authored-here code and from linked crates.

### Registering as `surrealds` — the finding that shaped the work

The suite's backend-name vocabulary already contains `surrealds`. Registering
under that name is what puts us behind the assertions the suite reserves for the
engine we are reimplementing, and it is the honest name for it.

It is also a **stricter** contract than registering under any other name, and the
specifics were not obvious:

| Suite assertion | For `surrealds` | Why it matters |
| --- | --- | --- |
| `multiwriter_same_keys_allow` | **required** | Overlapping blind writes to one key all commit; the last committer's value wins. No write-write conflict detection. |
| `multiwriter_same_keys_conflict` | ignored | That is the first-committer-wins model, and it is *not* ours. |
| `snapshot::write_skew_permitted` | ignored | The suite documents the distributed store as serializable, so permitting the anomaly is not asserted of us. |
| `raw::getu_unsupported` | **required** | `getu` must be refused with `UnsupportedLockedReads`. |
| `builder_surface::metrics_collectable` | **required** | `register_metrics` must return a non-empty set whose every name is collectable. |
| `transactions_local` | **required** | The "local" flag must be `true`; upstream's own comment says the enterprise distributed store reports `true`. |

That combination — blind writes never conflict, reads *are* validated at commit —
is a coherent model and not first-committer-wins, and it is what we now
implement. Choosing the easier registration name would have kept
first-committer-wins and the four `getu` tests, and would have been a claim about
the engine that is not true.

### The first run: what actually failed

Baseline, before any fix: **56 passed, 21 failed, 12 ignored.** The failures
clustered into exactly the seven places the Phase 0 code was a sketch:

| Failure cluster | Count | Cause |
| --- | --- | --- |
| `cancel` did not roll back | 3 | `set` wrote straight through; `cancel_discards_writes`, `commit_after_cancel_errors`, `savepoint::discarded_by_cancel` |
| No snapshot isolation | 1 | `snapshot::snapshot` — a reader saw a concurrent writer's value |
| `put` overwrote instead of refusing | 2 | `raw::put`, `defaults::replace` |
| Versioned reads silently ignored | 1 | `versioned::unsupported_error` |
| Conditional ops conflicted at *call* time | 4 | every `multi::` conditional test — both transactions saw the other's un-staged write and got `TransactionConditionNotMet` before either committed |
| Savepoints did nothing | 7 | `rollback_to_save_point` was a no-op; no `NoSavepoint` underflow |
| Cursor `Break` skipped rows | 2 | `for_each` advanced by `limit` instead of by rows visited |
| No metrics | 1 | `metrics_collectable` |
| `local` flag inverted | 1 | `transactions_local` — we returned the *distributed* flag where the contract wants *local* |

The conditional-op cluster is the instructive one: the pre-existing code failed
these tests for the *wrong* reason. It reported a conflict when both
transactions had merely read the same value, which is not a conflict at all —
neither had committed. The suite then failed later tests because there was no
commit-time check to reach.

### What changed

- [x] `storage.rs` rewritten as a **versioned** keyspace: per-key version lists
      with tombstones, a stamp handed out under the same lock that appends, and a
      commit history for range validation. Validation and application happen under
      **one** lock, so nothing can be written between the check and the write.
- [x] `txn.rs` rewritten: staged writes (so `cancel` discards by construction),
      read-your-writes through the staged write set, a **recorded read set**
      validated at commit, and undo-log savepoints with nesting and
      release-merges.
- [x] Conflict model: a write whose read set moved since the snapshot is refused
      with `Error::TransactionConflict`, which upstream marks retryable (R-0012).
      A blind write never conflicts with another blind write; both commit and
      serialise in stamp order (R-0034).
- [x] All read paths reject a `version` argument with
      `UnsupportedVersionedQueries` rather than answering from the wrong version.
- [x] Seven hand-written overrides deleted in favour of the **trait's own
      defaults**: `replace`, `clr`, `clrc`, `delr`, `clrr`, `batch_keys`,
      `batch_keys_vals`. They were doing the same work with weaker
      closed/read-only checks; the defaults encode the contract correctly, and
      `delr`/`clrr` in particular are where a hand-rolled version forgets to
      report `TransactionReadonly` on a read-only transaction. `getm`, `getr` and
      `count` stay overridden — each takes the store lock once and records the
      read it performed, which the defaults would do key by key.
- [x] Cursors fixed: a `limit == 0` call consumes nothing and exhausts nothing; a
      visitor `Break` consumes exactly the rows it saw and counts them, so the
      next call resumes after the last one.
- [x] `register_metrics` publishes six `surrealdb.ds.*` counters, each backed by a
      counter the storage tier maintains. An undeclared name returns `None`, not
      `0` — a missing series and a flat one are different facts.
- [x] The "local" flag is now the local flag.

### What the suite does not check for us

Recorded so a green run is not read as more than it is.

1. **`getu` conflict detection (6 tests ignored).** `raw::getu_unsupported`
   *requires* our refusal, so we are asserted on — but the four `multi::getu_*`
   tests that would exercise the guarantee are reported ignored. We have no
   locked reads at all. `SELECT … FOR UPDATE` inherits read-set validation
   because plain reads already get it, which is a stronger property than a row
   lock, but it is not the same code path and nothing tests the difference.
2. **Serializability (1 test ignored).** The suite ignores
   `write_skew_permitted` for `surrealds` precisely because it expects us to
   prevent the anomaly — which leaves nothing in the conformance run holding us to
   it. `crates/surrealdb-ds/tests/serializable.rs` therefore asserts it directly:
   write skew is refused, the refusal is retryable, a refused transaction can be
   re-executed and commit, disjoint writes both commit, a read-only commit never
   conflicts, and a concurrent write *inside a scanned range* is a conflict.
3. **`compact` (1 test ignored for us).** We report `CompactionNotSupported`,
   which is the suite's expectation for backends without a compaction primitive.
   Correct today, and a Phase 1 obligation to stop reporting.
4. **First-committer-wins (1 test ignored).** Deliberate: we are not that model.
5. **Three tests that are not about any external backend at all**
   (`compact_supported`, `transactions_remote`,
   `destroy_range_empties_the_range` — all `only = [rocksdb]` or
   `only = [tikv]`). Nothing to do here; they are counted only so the 12 adds up
   to 9 + 3.

So: 6 + 1 + 1 + 1 + 3 = 12.

### Open questions carried forward — one answered

1. ~~Can `surrealdb-kvs-test` be vendored on its own?~~ — **answered 2026-10-02:
   yes, it is cheap.** Four dependencies, none of them workspace-wide.
2. ~~Does `surrealdb-server` expose a public init path accepting a
   caller-constructed registry?~~ — **answered 2026-10-02: yes.**
   `surrealdb_server::init` takes one generic composer implementing
   `TransactionBuilderFactory + RouterFactory + ConfigCheck +
   ObservabilityProvider`, and `TransactionBuilderFactory` is the seam: its
   `CommunityComposer` impl is a three-line delegation to
   `Backends::community()`, so a composer that builds `Backends::community()`,
   registers `DsBackend`, and returns `TransactionBuilderParts::without_router_state`
   is the whole of it. Recorded as R-0036. **Not yet wired up** — see below.
3. Does v3.3.0 still carry the `safe_timestamp` contract? **Yes, confirmed
   2026-10-02.** `Transactable::safe_timestamp`'s doc comment names SurrealDS by
   name: a distributed backend whose commit log is non-linear MUST override it or
   the live-query router misses notifications (R-0010). Nothing in the suite can
   check this — the test only asserts the watermark does not exceed a freshly
   minted stamp, which a single node satisfies trivially.
4. Exact precondition semantics for `putc`/`delc` with `chk = None`: **settled by
   the suite** (R-0035). `None` asserts the key is *absent*, not "anything goes",
   which is what makes a conditional create atomic even on a last-writer-wins
   backend. Our implementation had it right by accident; it is now right by
   construction and asserted.

### Continuous integration

- [x] `ci.yml` — every push and PR. Two jobs: the conformance suite alone (fast,
      four small dependencies) and the whole workspace with clippy at
      `--deny warnings` (slow, and the one that proves ADR-0003 — our binary
      carries the engine, not a patched upstream one). Runs `--locked`, so a green
      result means "against the Cargo.lock in this commit".
- [x] `upstream-drift.yml` — nightly. Resolves against whatever is newest on
      crates.io, re-runs the suite, diffs the published crate inventory against the
      committed one, and **files an issue** on any failure. This is the workflow
      that matters most: `surrealdb-kvs` says its API "is free to change and break
      code even between patch versions", so the real risk is upstream drift, not a
      bug we write. It deliberately does *not* pass `--locked`.
- [x] `scripts/locked-versions.sh` — reports the resolved `surrealdb-*` versions,
      so a failing drift run says *what moved* rather than just that something
      did. It exits non-zero if the pattern stops matching, because a helper that
      silently finds nothing would make the detector report "no version change"
      forever.
- [x] No third-party actions. Toolchain pinned to 1.95.0, the version the suite is
      verified against, so a rustc bump cannot be mistaken for a contract change.
      `CARGO_FLAGS` was added to the Makefile so CI can pass `--locked` without
      changing anyone's local invocation.
- [x] Every `run:` block was extracted from the YAML and executed locally against
      stubbed inputs before being committed. That found one real bug: the "no
      version change" fallback was written `diff ... || echo`, and `diff` exits 1
      *when the files differ*, so the message printed exactly when it was false.

### Next action

Wire the composer (R-0036) and serve HTTP: `/health`, `/ready`, `/version`, then
SurrealQL over `/rpc`. That is the last Phase 0 task, and answering the open
question unblocked it — the seam is a single generic parameter.

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
- [x] **DECIDED: Path A** (2026-10-01) — link SurrealDB's crates and use them
      wherever appropriate; write only the distributed storage engine. See
      ADR-0002, now *accepted*. The repository layout already assumed this, so no
      restructuring was needed.
- [x] Confirmed the scope of what we link vs. what we write. Upstream stays
      authoritative for the front end and the storage contract; we own
      consensus, replication, recovery, the storage tier, node membership and
      `surrealdb.ds.*` telemetry.
- [x] Recorded the design consequence that matters most under Path A: keep
      proprietary product logic out of crates that link SurrealDB, and prefer a
      process boundary between them.

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

- ~~`TransactionBuilder` at v3.3.0 has **no `extension()` hook**.~~ **Wrong, and
  corrected on 2026-10-02 (R-0031).** It has one — `extension(TypeId)`, plus
  `wait_until_serve_ready()` — both defaulted, the default returning `None`. The
  mistake was reading "our implementation does not override it" as "the trait has
  no such method", which a defaulted method makes indistinguishable from outside.
  Phase 4's bucket/object-store work does go through `BucketStoreProvider` in
  `surrealdb-core`, which is a separate composer-level hook and unrelated to
  `extension()`.
- `Backends::new_transaction_builder` does not return `TransactionBuilderParts`;
  it returns the boxed builder directly, so there is no router-state threading at
  the registry layer in v3.3.0.

### Honest state

The extension point is real, open, and works from an external crate. That was the
single biggest risk in the project and it is now retired.

What is **not** done, and should not be assumed:

- Storage is `BTreeMap`-backed and **in-memory**. No durability, no persistence.
- Transactions are snapshot-isolated with read-set validation, but the whole
  cluster is **one process and one `Mutex`**. Nothing here survives a crash, and
  there is no replication.
- `getu` (locked read) returns `UnsupportedLockedReads`, so `SELECT … FOR UPDATE`
  has no conflict guarantee of its own.
- `safe_timestamp` is the single-node default. **Unsafe on more than one node.**
- `compact` declines rather than lying about having done work.
- The upstream conformance suite has **not** been run — it needs the vendored
  tree. **Done 2026-10-02; see the entry above.**
- The HTTP surface is not wired up; the binary constructs an engine and exits.

Next action (as of 2026-10-01): vendor `surrealdb-kvs-test` and get our engine
through the conformance suite. That is the real Phase 0 exit criterion in
PLAN.md. **Done 2026-10-02.**

---

## Open questions carried forward

*Superseded by the 2026-10-02 entry above, which answers questions 2–5. Kept as
written at the time so the record shows what was open when.*

1. ~~ADR-0002 licensing decision~~ — **resolved 2026-10-01: Path A.** No longer
   blocking.
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