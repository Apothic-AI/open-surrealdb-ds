# Progress log

Dated, append-only. Newest at the top. An entry records what was *actually
verified*, not what was intended.

Status legend: `[x]` done and verified · `[~]` in progress · `[ ]` not started ·
`[!]` blocked

---

## 2026-10-05 — ADR-0012 step 1: a directory we write is one upstream can open and read correctly

The **write** half of on-disk interop now works, as a deliberately small probe.
`crates/surrealdb-ds-server/tests/interop.rs` gains a write direction: our own
`rocksdb` binding authors a directory, upstream's reader opens it, and a
round trip through it is byte-faithful. `make interop` is now **9 tests**.

```
write: 7 live keys / 108 value bytes, 1 tombstone(s), no prefix extractor, 2 .sst file(s)
options: prefix_extractor=nullptr
read:  upstream opened our file and read all 7 keys byte-identically
range: person records 2, person index 1, person metadata 1, person edges 1,
       edge table records 1, cross-category 5, root metadata 1
       — each exactly the bounded slice of the full read
point: one key returns its value; the tombstoned key reads as absent
round trip: upstream wrote and deleted through our file; both readers agree on the net effect
```

**The right rows came back, which is the result. The successful open is not.**

### No prefix extractor, and it worked

ADR-0012 decided to write with **no** prefix extractor, using explicit key bounds,
because a wrong one silently widens reads and byte equality cannot detect it. That
prediction held: our `OPTIONS` records `prefix_extractor=nullptr`, every setting
in it differs from upstream's profile, and **every one of those differences was
inert** for openability and read correctness at this scale. `OPTIONS` is not
consulted on open at all (ADR-0011), so none of its recorded settings is required.

Upstream still installs `TablePrefix.v1` on *its* side and chose
`prefix_same_as_start` for itself on the in-domain same-prefix record range. So
the probe cannot avoid exercising that path — which is what makes the range
assertion the meaningful one rather than a formality.

### Minimum required, for step 6 to build on

One column family (`default`, no on-disk marker); key bytes verbatim; value bytes
verbatim; `create_if_missing(true)` and otherwise `Options::default()`. That is
the whole list.

### Verified on the final tree

- `make interop` → **9 passed; 0 failed**, `EXIT=0`
- `cargo remote-3000 -r fly -d 1.95.0 -- test --workspace` → `EXIT=0`:
  19 + 77 (12 ignored) + 6 + 2 + 3 golden + **9 interop** + 1 doctest
- `cargo remote-3000 -r fly -d 1.95.0 -- clippy --workspace --all-targets -- --deny warnings`
  → `EXIT=0`, zero warnings
- **Falsifiable, both directions.** The write probe's own comparison caught an
  injected flipped bit at byte 21 of 22 (`expected 65, found 64`), and the read
  probe's at byte 171 of 172. The corruption is applied to an in-memory copy, so
  no on-disk revert is needed.
- Engine crate untouched; `Cargo.lock` unchanged; no dependency added.

### What this does NOT prove

- **No tier.** Raw RocksDB with no `OPTIONS` profile, no WAL recovery, no
  reopen-after-crash, no commit semantics. This shows a reader can open what we
  write; it does not show our engine is equivalent.
- **No key class is proven complete** — the same limit the golden and read halves
  record.
- **Small.** One SST per flush, no compaction, so compression, blob files,
  two-level indexes and partitioned filters remain configured-but-unexercised.
- **Source-derived, not observed:** that upstream sets `prefix_same_as_start` on
  that range (R-0057) is read from `surrealdb-kvs-rocksdb`. What was observed is
  the row set, not the `ReadOptions`.

### Correction to my own documentation, found by this tranche

`AGENTS.md` documented a command that **does not work**:

```
cargo remote-3000 -r fly -d 1.95.0 build -c=debug/surrealdb-ds-server -p surrealdb-ds-server
error: invalid value 'surrealdb-ds-server' for '--ssh-port <PORT>'
```

cargo-remote parses its own flags *after* the cargo subcommand too, and `-p` is
its `--ssh-port`. The `--` terminator is what keeps cargo's `-p` away from
cargo-remote's. Verified both the failure and the fix; every example in
`AGENTS.md`, `docs/remote-builds.md` and the `shared-flyio-build-server` skill now
carries `--`. This is the second time a `-p` has collided with cargo-remote's own
flags in this project, and it will happen again to anyone who skips the
terminator.

### Next action

ADR-0012 **step 2**: restart-safe local persistence. Create/open/reopen, atomic
write batches, tombstones, WAL recovery, and behaviour after a crash at each
commit boundary. That is the first step that is about durability rather than
format, and crash recovery is the thing a readable directory cannot evidence.

---

## 2026-10-04 (later still) — On-disk interop, read half: our reader opens upstream's directory

**ADR-0008's read half is confirmed.** `make interop` opens a directory upstream's
own engine wrote, using `rocksdb` as *our* dependency, and reads back **55 keys /
1526 value bytes byte-identically** — every key and every value, no exclusions.

But three claims of mine were wrong, and the corrections matter more than the
confirmation. All recorded in ADR-0011.

### Correction 1 — a missing prefix-extractor name is not an open failure

ADR-0008 said `surrealdb.TablePrefix.v1` must be registered by that exact name or
the open *fails*. **It does not read the on-disk `OPTIONS` file at all.** The
directory opens with no extractor, and equally with one registered under a
deliberately wrong name. Both are asserted as tests, because the premise was
load-bearing and false (R-0051). The name in `OPTIONS` is inert.

### Correction 2 — the real failure mode is worse than an error

A wrong extractor does not fail. It **silently widens** the read: a
prefix-restricted seek returns the entire run instead of the narrowed range.

```
2 rows with our extractor, 24 with a mismatched one, 24 with none — wider, not an error
```

Same hazard class as this project's other correctness risks: a subtly wrong rule
producing **silently wrong data rather than an error**. Worse than the open
failure ADR-0008 predicted, because it will not announce itself.

### Correction 3 — byte equality cannot validate the extractor

Injecting an off-by-one into our extractor left the headline byte-equality test
**green**. Reading a whole keyspace and comparing it byte-for-byte cannot
distinguish a correct prefix extractor from a slightly-short one, because a
too-short prefix reads *more* rows and the comparison still matches. So the
extractor needs its own range assertions, which is why
`a_mismatched_extractor_silently_widens_a_prefix_restricted_read` exists.

### `TablePrefix.v1`, as measured

Variable-length, ending at and including a one-byte discriminator. In domain iff
≥14 bytes, `key[0,1,6,11]` are `/ * * *`, a `\0` at offset ≥12, and ≥1 byte after
it; prefix is `key[..null_pos + 2]`. `*` records, `+` index, `!` metadata, `~`
edges, `&` refs. Out-of-domain keys pass through unchanged and are excluded by
`InDomain`. 39 of our 55 keys are in domain. `SRC`, format shape only (R-0050).

### The binding was a cost decision, not a necessity

ADR-0010 implied the SurrealDB fork was required. **It is not.**
`format_version = 7` does not discriminate: `kLatestBbtFormatVersion` is 7 in
RocksDB 11.0.0, 11.8.1 and 10.4.2, read floor 2 in all three. We use
`surrealdb-rocksdb` 0.24.0-surreal.5 (RocksDB 11.0.0) because it is **already in
our `Cargo.lock`** — one edge, zero rebuild, against a full C++ rebuild of a
different RocksDB. Features pinned to `lz4`+`snappy`; defaults would add
zstd/zlib/bzip2/bindgen and force that recompile.

It is a SurrealDB-maintained fork, so `NOTICE` gained a fourth licensing path
rather than letting a crate that is neither vendored nor ours-by-name pass
unremarked.

### Verified, and falsifiable

- 55 keys / 1526 bytes, our reader == upstream's (`our_reader_opens_an_upstream_written_directory`).
- Wrong extractor widens rather than errors (2 vs 12 rows in the recorded run).
- Reader reads SSTs, not just the WAL; a clean shutdown rewrites its own `/!nd`
  row, byte 27 (R-0053) — which cost a round and would have looked like corruption.
- The comparison catches a flipped bit; the harness is falsifiable (injected
  `pop_last()` → 3 failures; injected extractor off-by-one → 2 failures; reverted).
- `make golden` unregressed at 55/1526, manifest byte-identical.

### What this does NOT prove

**Any writing.** ADR-0008's other half — a directory *we* wrote, opened by
upstream — is entirely untested. Also: key-class completeness; anything past one
L0 SST (compression, blob files, two-level indexes and partitioned filters are all
configured in `OPTIONS` and none exercised — `bottommost_compression=kZSTD` is
never reached, because upstream links only lz4 and snappy, so `OPTIONS` records
defaults, not behaviour); bloom-filter correctness at scale; and the Rust API's
stability.

### Next action

The local durable tier, with **"upstream's engine opens a directory we wrote"** as
its first milestone rather than a later integration test — it forces every format
question while the surface is still small, and it gives an unambiguous pass/fail.
Per Correction 3, that milestone needs range assertions on the prefix extractor,
not byte equality.

---

## 2026-10-04 (later) — Builds run on a shared Debian server; the local disk is no longer the constraint

Local `target/debug` was **32G against 18G free**, so local building had stopped
being an option rather than merely being slow. `cargo clean` now reclaims it
(net 7G — cargo's own 40.9GiB figure double-counts hardlinks from `deps/`), and
the box does the compiling.

### The server

New app `open-surrealdb-ds-builder`: `debian:bookworm-slim` on `performance-8x`
(8 dedicated cores, 16 GB), 100 GB volume at `/data`, reached over **2PN** only —
no public port of any kind. A *separate app* because the pre-existing
`fly-builder-glowing-aurora-7598` is a registered org build machine, where Fly
refused both an image change (*"deploying over the remote builder is not
allowed"*) and a second machine (*"remote builders may have only one volume"*).

Debian is a hard requirement, not a preference — see the ADR-0009 entry below.

**Multi-project by construction.** `/data/cargo` and `/data/rustup` are shared, so
the crates.io cache and the toolchain are paid for once; each project gets its own
tree under `/data/builds`, named by cargo-remote after a hash of the project path,
so projects cannot collide and need no agreed name; and one build gets
`[build] jobs = 4` of 8 cores so two projects divide the machine instead of
starving. No project-specific state lives on the box.

### Verified remotely, all exit 0

```
cargo remote-3000 -r fly -d 1.95.0 check  --workspace --all-targets    10m29s
cargo remote-3000 -r fly -d 1.95.0 test   --workspace                  green
cargo remote-3000 -r fly -d 1.95.0 clippy --workspace --all-targets \
    -- --deny warnings                                                  clean
  test: 19 + 77 (12 ignored) + 6 + 2 + 3 golden + 7 interop + 1, 0 failed
```

The whole tree is **3.8G on the box against 32G locally**, because debug info is
off there and nothing is debugged on a build server. `-d 1.95.0` is mandatory:
cargo-remote defaults the toolchain to `stable` and runs `rustup default` on every
build, and there is no config-file key for it — omitting it silently installs and
switches compiler versions, which must never be mistakable for a contract change.

### Consequence for how we work

`make check` and `make test` still call local `cargo` and remain the commands of
record, because they are what CI runs. Remote builds are the fast path, not the
definition of done. `make smoke` needs a local binary, which now takes an explicit
`--copy-back`; it is the one command with no zero-artifact form.

`AGENTS.md` is new, and the operational reference for the server is the
`shared-flyio-build-server` agent skill, which carries the machine definitions and
provision scripts — they describe a shared environment, so every project would
otherwise carry a copy that drifts.

### Baseline intact

`make check` clean · `cargo clippy --workspace --all-targets -- --deny warnings`
clean · `make test` 19 + 77 + 12 ignored + 6 + 2 + 3 + 1, 0 failed ·
`make smoke` 10/10 against `ds+mem://` · `make golden` 55 keys / 1526 bytes ·
`make interop` 55 keys / 1526 bytes.

---

## 2026-10-04 — L2 is evidenced: 55 keys round-trip upstream → us → upstream, byte for byte

**The project's stated definition of 1:1 is no longer an untested assumption.**
`make golden` drives a real SurrealQL workload through a real `Datastore` against
upstream's own `rocksdb:` engine, dumps the whole keyspace, replays those exact
bytes into our in-memory tier, reads them back, and replays our output into a
second upstream store as a control.

```
$ make golden
round trip: 55 keys / 1526 value bytes survived upstream -> ds+mem:// -> upstream
golden: upstream and our engine both match …/tests/data/golden-3.3.0.txt
  # 35 of 55 keys pinned, carrying 1060 of 1526 value bytes
versions: both tiers agree across 9 steps
test result: ok. 3 passed; 0 failed
```

### The premise of Phase 1 was wrong, and that is the headline

Phase 1 opened with *reconstruct the key encoding contract from the published
interface* (`surrealdb-kvs`'s `key`/`value` modules). **There is nothing to
reconstruct.** That crate's own `lib.rs`: *"This crate defines how a key and a
value is **declared**; it does not declare any."* `KVKey::encode_buffer` and
`KVValue::kv_encode_value` are traits; the layout is declared one level up by
`surrealdb-datastore`'s `keyspace!` invocation, encoded by
`surrealdb-keyspace-macro` — all published, all already in our lock file.
Recorded as R-0044. PLAN.md's task is deleted rather than attempted.

And because `Transactable` is byte-level — a key *is* a `Cow<[u8]>` — byte
compatibility is **faithful passthrough** (R-0045). Writing our own encoder would
have been strictly worse: more code, more risk, and a busier reading of the
clean-room line for no gain.

### The oracle was already in our binary

`surrealdb-kvs-rocksdb` and `surrealdb-datastore` were in `Cargo.lock`
transitively via `surrealdb-server`, so `construct-only rocksdb:<path>` runs
upstream's real engine and writes upstream's real files — verified, not assumed.
A "dataset written by upstream" costs no second checkout and no Enterprise
licence. Separately, `surrealdb_server::core` reaches `surrealdb_core`, so
`core::kvs::Builder::build_with_factory_path` builds a `Datastore` against any
registry in-process: no argv, no global tracing subscriber, no version check, no
socket (R-0046).

### Replay, don't regenerate — the one decision that was load-bearing

The harness was first written to *generate* the dataset on both tiers and
compare. Measured: the same workload run twice against upstream's own `rocksdb:`
gives 55 keys of which only **35** are identical; the other 20 mint a fresh
`Uuid::new_v7()` per run. Masking them is worse than leaving them — `new_v7()`
embeds a millisecond timestamp, so runs share high bytes and a pinned subset goes
stale within minutes. So the harness **copies**: author once, dump, replay those
bytes through us, then replay our output back into upstream as a control. Every
key and value is compared with no exclusions, because the compared bytes were
copied rather than independently regenerated.

### What the dataset actually contains

Not a toy, and checked by decoding the pinned keys rather than by intent:
graph edge documents plus the `~` keys on **both** endpoints, a unique index and
its `+{ix}` entries, an **HNSW vector index** with its `!hr` graph-layer records
and serialised `[1.0, 2.0, 3.0]` vectors, record documents, doc-id mappings,
table and field definitions, and a tombstone. A second test walks both tiers
through nine steps — absent → v1 → reader pinned at v1 → v2 → delete → recreate
over tombstone → scan — and asserts the pinned reader still reads v1. That is the
case most likely to expose a bug in our version lists, and the tiers agree.

### Falsifiability, verified rather than asserted

A single flipped bit injected into `DsTxn::set` for 172-byte values was tried
twice and reverted. Both probes failed **two of the three** golden tests, at the
injected offset, with the injected mask:

| Probe | Injected | Key hit | Reported |
| --- | --- | --- | --- |
| 1 | `if val.len() == 123 { val[7] ^= 0x01; }` | `*knows*edge_one` | byte 7, `expected 00, found 01` |
| 2 | `if val.len() == 172 { val[100] ^= 0x01; }` | `*person*one` | byte 100, `expected 6d, found 6c` |

I re-ran probe 2 independently rather than trusting the report, and confirmed the
value is 172 bytes and byte 100 is the terminating `m` of `"one@example.com"`.
`git diff crates/surrealdb-ds/` is empty. **A harness that cannot fail is
decoration**, and this one demonstrably can.

### What this does NOT prove

1. **No key class is proven complete.** Only the classes this workload reaches. A
   class upstream writes that it never touches is untested, and *no failure would
   announce it*.
2. **The manifest pins 35 of 55 keys, not all.** The 20 unpinned carry a per-run
   UUID — `!tb{tb}`, where `PERMISSIONS` lives, among them. Their bytes are still
   compared absolutely by the round trip; they are simply not in the committed
   file.
3. **No multi-version count is verified.** The KV surface cannot report one; our
   tier refuses `version` arguments. The version leg compares observable
   behaviour differentially, not version bytes.
4. **In-memory only.** Durability, recovery, and the on-disk format are Phase 1
   task 2 and are untouched.
5. **Upstream's RocksDB was unversioned throughout.** A plain `rocksdb:` path
   installs neither the UDT comparator nor `set_timestamp(u64::MAX)` —
   `RocksDbConfig::versioned` defaults to `false` (R-0048), confirmed in the
   `OPTIONS` file as `comparator=leveldb.BytewiseComparator`. So this says
   nothing about the `surrealdb.TimestampComparator` path, which is opt-in behind
   `?datastore_versioned=true`.

### Two corrections to my own briefing

The delegated agent found an error in the brief I wrote it, and was right:

- I asserted the on-disk format *is* the UDT layout. It is opt-in; I had
  verified it against an unversioned open. R-0048.
- I mis-decoded a key as `/*{ns}!db{db}person*…`; it is `/*{ns}*{db}*person\0*\x03one\0`
  — four `*` separators, no `!db`.

It also found that **our refusal of versioned reads is upstream's own position**:
`surrealdb-kvs-mem` rejects `datastore_versioned` at startup with
`UnsupportedVersionedQueries` (R-0049). That reframes PLAN.md's versioned-read
task from a conformance gap into a design decision — implementing time-travel
would make our tier *diverge* from upstream's local tier.

### Baseline intact

`make check` 0 warnings · `cargo clippy --workspace --all-targets -- --deny warnings`
clean · `make test` 19 + 77 + 12 ignored + 6 + 2 + **3** + 1, 0 failed ·
`make smoke` 10/10 against `ds+mem://` over HTTP. The engine crate
`crates/surrealdb-ds/` is **untouched** by this work, and `storage.rs` still
never imports `surrealdb_kvs`.

### Next action

Phase 1's durable backend, with the scope question ADR-0007 deliberately leaves
open: whether L2 means our files must be interchangeable with upstream's, or
only that our tier preserves the bytes it is handed. Nothing here answers it.

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

---

## 2026-10-02 (later) — Version GC, and a poisoned store that tells the truth

Two defects closed. `make check`, `make conformance` (77 passed / 12 ignored),
clippy at `--deny warnings`, and 19 storage/builder unit tests are green.

### Version GC — the unbounded-growth defect

`storage.rs` grew for the life of the process: every committed version of every
key forever, plus a `history` deque of every committed `(stamp, key)` pair that
existed only so range validation could avoid walking the range.

- [x] A transaction **pins** its snapshot stamp for its whole life. The pin lives
      in `DsTxn` as `Mutex<Option<SnapshotPin>>`, so dropping a transaction
      without committing or cancelling releases it — the case a commit/cancel-only
      scheme leaks.
- [x] Collection retains, per key, **the newest version at or below the oldest
      pinned stamp, plus everything above it**. That is sufficient, and the
      argument is in the file: for any live snapshot at or above the horizon,
      every version at or below it is shadowed by the retained one. With nothing
      pinned the horizon is the clock, which collapses each key to one version.
- [x] `history` **deleted**, not bounded. Range validation now checks each key's
      newest stamp, which costs one pass over the range instead of the number of
      commits since the snapshot. The file's `# Known limits` section says so
      plainly rather than pretending the trade is free.
- [x] Nine new tests. The load-bearing one is
      `a_pinned_snapshot_still_reads_after_collection`: pin after `v2`, write
      `v3` and `v4`, collect, then assert `v1` is gone, `v2` survives, the pinned
      reader still reads `v2`, and a fresh reader reads `v4`.
      `pinned_reads_match_the_full_history_under_concurrency` checks the same
      property against full history while writers run.

### The hole, named rather than papered over

A leaked transaction — `mem::forget`, or a retained `Box<dyn Transactable>` —
holds the horizon for ever and **stops collection entirely**, which is the
behaviour GC replaced. A leaked `Box` is not hypothetical: `TestDs::transaction`
returns one, and upstream's own comment says dropping a cursor matters because
some backends block on it. The fix is a query timeout plus an engine that lets
the query layer drop its handle; **neither exists yet**, and the file says so.

Collection is also an O(keyspace) walk under the store lock on each horizon
advance. Documented as a limit; a real engine collects from its local store's
compaction, off the request path.

### A poisoned store now says so

The old `lock()` recovered from poisoning on the strength of a comment claiming
the clock and the map "are only ever updated together". **That was false**: the
clock advances *after* the write loop, so a panic inside it leaves some keys
carrying a version at a stamp the clock never reached, and the next commit
computes the same stamp and pushes a second version for those keys. Not a
"poisoned warning" — a lost-write shape.

Found while reviewing a delegated attempt, not by a test.

The fix is to stop claiming what we cannot guarantee. A poisoned store now marks
itself and **serves nothing, ever**: `StorageError::Poisoned`, surfaced as
`Error::Internal`. `DsTxn` remembers if it was born on a poisoned store and
refuses every operation. Silently serving reads from a keyspace whose versions
and clock can disagree is the same class of lie as `compact` claiming work it
did not do.

An intermediate version of this had a dead `Err` match arm that made the poison
unreachable, so the flag was never set and the crate did not compile under
`--deny warnings`. Caught by clippy, not by a test.

---

## 2026-10-02 (later still) — Phase 0 closed: SurrealQL over HTTP on our engine

`make smoke` starts the server on `ds+mem://` and passes ten checks. This was
Phase 0's last task.

```
$ make smoke
  ok   server is up
  ok   /health -> 200
  ok   /ready -> 200
  ok   /version -> surrealdb-3.3.0
  ok   namespace and database defined over /rpc
  ok   CREATE over /rpc
  ok   SELECT over /rpc
  ok   the created document is readable back with its value
  ok   an unknown RPC method is reported as an error
  ok   an unknown database is refused, not silently accepted

smoke: all checks passed against ds+mem:// over HTTP
```

### The seam turned out to be one generic parameter

`surrealdb_server::init` takes a composer implementing `TransactionBuilderFactory
+ RouterFactory + ConfigCheck + ObservabilityProvider`, and upstream's own
`CommunityComposer` — a public unit struct — satisfies all four. So
`crates/surrealdb-ds-server/src/composer.rs` **delegates three traits and
overrides exactly one**:

- `TransactionBuilderFactory` — our `Backends::community()` + `register(DsBackend)`.
- `RouterFactory` — one call to `community_router(&[])`, which is public and is
  what `surreal` itself serves.
- `ConfigCheck`, `ObservabilityProvider` — delegated.

That last one is the whole reason this was small: `/health`, `/ready`, `/version`,
`/rpc`, `/export`, `/import`, `/status`, `/grpc` and `/sync` come from one public
function rather than from anything we wrote. Reimplementing any of it would be
reimplementing the front end, which ADR-0002 says not to do.

### Three things that were not obvious, and each one cost a round trip

1. **`init` builds its own tokio runtime and `block_on`s.** Our `main` was
   `#[tokio::main]`, so calling `init` from inside it panicked with "Cannot start
   a runtime from within a runtime". `main` is now synchronous, and the
   construct-only path builds a short-lived runtime of its own.
2. **We must not claim the global tracing subscriber.** Upstream's CLI installs
   its own from `--log`; two `set_global_default` calls panic. Tracing is now
   initialised only on the path that does not hand argv to `init`.
3. **`/rpc` wants a JSON-RPC object, not bare SurrealQL**, and namespaces and
   databases are not auto-created. Recorded as R-0042.

### The smoke test asserts things a stub would fail

Two of the checks exist specifically to catch a fake:

- **An unknown database is refused.** `RETURN 1` is answered from the expression
  without resolving the database, so it returns OK against *any* database name and
  would make this check vacuous; the check uses a real `SELECT`. This is the one
  that proves a real keyspace is being consulted, through our engine.
- **An unknown RPC method errors.** A front end that swallowed errors would pass
  every round-trip check above it.

It asserts on HTTP status and response bodies, never on the server's stdout —
a debug build prints a banner and the CLI prints a version-check result, and
asserting on either would make it brittle.

### Consequence: this binary is `surreal`

`init` boots the upstream CLI over `std::env::args()`, so
`surrealdb-ds-server start [options] <path>` and `version` and `config` are
upstream's, not ours (R-0041). `construct-only <path>` is the one addition: it
builds the registry, constructs, reports and exits, which keeps the Phase 0
registration proof non-blocking now that `run` serves by design.

### Phase 0 is closed

All eight tasks, and all three exit-criterion commands green:

```
make check      # all crates type-check
make test       # includes the conformance suite: 77 passed / 12 ignored
make smoke      # SurrealQL over HTTP on our engine
```

CI gained a third job so the HTTP claim is checked on every push, not just when
someone remembers to run it.

### Next action

Phase 1: the key encoding contract and a durable local backend. **Nothing so far
is durable and L2 — byte compatibility with a real SurrealDB node, which is this
project's stated definition of 1:1 — is unproven.** That is now the largest
remaining risk, and it is bigger than anything left in Phase 0.

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