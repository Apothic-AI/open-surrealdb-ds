# Architecture decision records

Newest last. Each record states the decision, the alternatives that were
rejected, and what would change our mind.

---

## ADR-0001 — Target the v3.3.0 release tag, not the repository default branch

**Status:** accepted · **Date:** 2026-10-01

### Context

The first pass of our architecture analysis read the SurrealDB repository's
default branch. That turned out to be the wrong tree:

- `origin/main` at commit `18971ff` is dated **2026-09-04**.
- Tag `v3.3.0` at commit `238bfeb` is dated **2026-09-24** — twenty days later.
- `git merge-base --is-ancestor v3.3.0 origin/main` → **false**. They are on
  divergent lineages, and `main`'s log interleaves two unrelated PR numbering
  series (`#7368` alongside `#534`), which indicates a rewritten or merged history.

The two trees have materially different architecture:

| | `origin/main` (pre-release) | `v3.3.0` (released) |
| --- | --- | --- |
| Workspace members | 11 | ~40 |
| KV layer | `surrealdb-core/src/kvs/` module | `surrealdb-kvs` crate |
| Backend registry | hard-coded `match` in a composer | `surrealdb-kvs-any::Backends` + `BackendProvider` |
| Conformance suite | none | `surrealdb-kvs-test` |
| Engine interface | composer traits | `surrealdb-engine-api` |

We confirmed which tree is the release by checking crates.io: `surrealdb-kvs`,
`surrealdb-kvs-any`, `surrealdb-kvs-rocksdb`, `surrealdb-engine-api`,
`surrealdb-datastore` and `surrealdb-catalog` are all published at `3.3.0`, and
they only exist in the modular layout. The `main` layout has no such crates.

### Decision

Pin everything to **tag `v3.3.0`** (commit `238bfeb`). `scripts/bootstrap.sh`
fetches that exact ref.

### Consequences

- The extension seam we target is the *documented, published* one, not a
  pre-release internal abstraction.
- Our first architectural analysis (recorded in `docs/research/`) was performed
  against pre-release code and has been corrected. See ADR-0004.

### Revisit if

SurrealDB cuts a release that again restructures the KV layer, or if we need a
capability that only exists on a later branch. Re-run the crates.io check in
ADR-0004 before trusting any layout assumption.

---

## ADR-0002 — Linking SurrealDB's crates makes our engine BUSL-encumbered

**Status:** **accepted — Path A chosen** · **Decided:** 2026-10-01

### Context

SurrealDB is **not** open source. The repository `LICENSE` is the **Business
Source License 1.1**, with:

```
Additional Use Grant: You may make use of the Licensed Work, provided that you
                      may not use the Licensed Work as a Database Service.
Change Date:          2030-01-01
Change License:       Apache License, Version 2.0
```

"Database Service" is defined as a commercial offering in which the Licensed
Work provides database functionality to third parties that lets them create,
manage or control schemas or tables. Direct employees and contractors working on
our behalf are carved out.

Every published SurrealDB crate carries `license = "non-standard"` on
crates.io, which resolves to BUSL-1.1. The Apache-2.0 badge rendered in
SurrealDB's own `CARGO.md` is inaccurate and should not be relied on.

BUSL-1.1 contains **no non-compete clause and no reciprocity/clawback**. It
restricts *use of the code*, not independent development of the same behaviour.
So writing an independent engine is permitted. What it does not permit is
laundering BUSL code into an Apache-2.0 or commercial competing product.

### The fork in the road

**Path A — interoperable, BUSL-encumbered.**
Implement `surrealdb_kvs_any::BackendProvider` and compile against
`surrealdb-kvs` / `surrealdb-kvs-any` / `surrealdb-server`.

- Leverage: the entire SurrealQL front end, planner, indexes, permissions,
  live queries, and the upstream `surrealdb-kvs-test` conformance suite come
  free. A working engine in weeks, not months.
- Cost: any crate that links them is *"subject to this License"*. We would
  relicense those crates BUSL-1.1. Acceptable for internal use, research, or a
  non-competing open-source release. **Not** acceptable if this is ever meant to
  be sold as a competing product.
- Note the upside: BUSL converts to Apache-2.0 on **2030-01-01**, roughly three
  years out, after which the encumbrance dissolves.

**Path B — independent, Apache-2.0.**
Zero SurrealDB code dependencies. Implement the storage engine *and* the
key/value encoding contract from scratch.

- Cost: no free front end, no free conformance suite, no published interface
  contract to code against. Substantially more work.
- Benefit: unencumbered, sellable, competing-capable, and the Apache-2.0
  licence is honest end to end.

### Licence history (verified 2026-10-01 via the upstream `LICENSE` file history)

| Date | Commit | Terms |
| --- | --- | --- |
| 2016-02-26 → 2016-05-11 | `d74af9681`, `2ad46f5f3` | Apache-2.0 boilerplate, predating SurrealDB's public release |
| **2021-12-14** | `5c4b9f83d` "Add license for SurrealDB 1.0.0" | **BUSL-1.1 from the first public release.** Change Date `2026-01-01` |
| 2023-04-01 | `54d285f1e` "Update license change date" | Change Date → `2027-04-01` |
| 2024-08-22 | `5bf311955` (#4582) | Licensed Work → "SurrealDB 2.0"; Change Date → `2029-09-17`; *Database Service* definition broadened |
| 2025-12-17 | `a504d2d1b` (#6683) | Licensed Work → "SurrealDB 3.0"; Change Date → `2030-01-01`; *Database Service* broadened again |

Three things this history tells us:

1. **BUSL has applied since day one.** SurrealDB has never been open source, and
   the Apache-2.0 badge in its `CARGO.md` is not evidence of anything.
2. **The Change Date is not a reliable planning assumption.** It has moved twice,
   each time later, and has been extended past the nominal four-year mark. Treat
   the encumbrance as open-ended.
3. **The *Database Service* definition has broadened with every major release** —
   from "creating tables whose schemas are controlled by such third parties"
   (1.0) to "creating or managing" (2.0) to "any product, service, platform, or
   commercial offering … to provide database functionality to third parties"
   (3.0). This is the clause we are most exposed to under Path A, and the trend
   runs against us.

### Decision

**Path A. Link SurrealDB's crates and use them wherever it is the right tool.**

The project links the published interface and builds only what SurrealDB does
not provide — the distributed storage engine. In practice that means:

| Link and use | Write ourselves |
| --- | --- |
| `surrealdb-kvs` — the `Transactable` / `TransactionBuilder` contract | Consensus: leaderless quorum, epochs, view change |
| `surrealdb-kvs-any` — `BackendProvider`, `Backends` registry | Replication: catch-up, anti-entropy, bounded recovery drain |
| `surrealdb-datastore`, `surrealdb-catalog` — keyspace, schema | Storage: local durable engine, object-storage tier |
| `surrealdb-core` — the query layer, reached through `surrealdb_server::core` | |
| `surrealdb-keyspace-macro` — the `keyspace!` codegen behind the key types | |
| `surrealdb-cnf` — configuration | Node membership and endpoint resolution |
| `surrealdb-server` — HTTP/RPC/CLI surface | Telemetry under the `surrealdb.ds.*` scheme |
| `surrealdb-engine-api` — the engine interface | |
| The whole query layer: SurrealQL, planner, HNSW/DiskANN, graph, permissions, live queries | |

This is the leverage-optimal choice and it is what the current crate layout
already assumes. No restructuring needed.

**"Wherever appropriate" is the operative phrase**, and it has a concrete
meaning: upstream remains the source of truth for the front end and the storage
contract; we own the distributed storage engine. We do not reimplement anything
that already exists and works.

### Consequences

- **Every crate here that links SurrealDB code is BUSL-1.1**, declared in its
  `Cargo.toml`. `LICENSE` (Apache-2.0) covers only code authored in this
  repository. `NOTICE` states the boundary in full.
- **Anything we link cannot be relicensed.** If the commercial goal ever changes,
  or a component needs to be closed-source, the fix is to move it behind a
  process boundary rather than to unwind the link.
- **Design for separability from the start.** Keep proprietary product logic out
  of crates that link SurrealDB. The cheapest structure is: our BUSL-licensed
  engine as a separate process, product logic talking to it over the network —
  which is also just how SurrealDB is deployed normally.
- **The Change Date is not a planning assumption.** It has moved twice, each time
  later, and past the nominal four-year mark. **Do not plan around 2030-01-01.**
  Re-read the LICENSE parameters on every upstream major upgrade;
  `make audit-upstream` is where that check lives.
- **Path B stays open.** `consensus`, `replicate` and `storage` hold no SurrealDB
  imports by design, so a later move to a fully independent implementation costs
  the boundary types, not the protocol. That is insurance, not a plan.

### Revisit if

- The project needs to sell anything as a *database*, or offer customers schema
  control — the *Database Service* clause, not the licence mechanics, is what
  blocks that.
- A component must be closed-source for commercial reasons.
- Upstream relicenses to Apache-2.0, which would remove the encumbrance and make
  a full re-evaluation worthwhile.

---

## ADR-0003 — Build a binary; do not patch the upstream CLI

**Status:** accepted · **Date:** 2026-10-01

### Context

SurrealDB's `surreal` binary hard-codes its set of storage backends. There is no
plugin discovery, no dynamic loading, and no environment hook that lets an
external crate register a backend into an already-built binary. The registry is
`Backends`, and `Backends::community()` returns only the first-party `kv-*`
providers compiled in.

### Alternatives considered

1. **Fork the upstream repo** and add our crate as a workspace member. Maximum
   access to internals, but immediately re-architects the tree, and every
   upstream release becomes a merge. Rejected for now.
2. **Link the crates and ship our own binary.** Verified in Phase 0:
   `surrealdb-ds-server` builds a `Backends` from `community()`, calls
   `register(DsBackend::new())`, and constructs through our scheme. The open
   question — whether `surrealdb-server` exposes an init path that accepts a
   caller-built registry — was **answered 2026-10-02**: it does, and the seam is
   one generic parameter. `surrealdb_server::init` takes a composer implementing
   `TransactionBuilderFactory`, and upstream's own `CommunityComposer` impl of
   that trait is a three-line delegation to `Backends::community()`. See R-0036.
3. **Patch the upstream binary.** No supported seam. Rejected.

### Decision

**Option 2.** We ship `surrealdb-ds-server`, our own binary, which builds the
backend registry with our provider registered and hands it to the server's init
path. No upstream source modification.

### Consequences

- We control our own version and release cadence; upstream upgrades are a
  dependency bump.
- Anything genuinely requiring private upstream internals becomes a later,
  isolated decision.
- Verified: at v3.3.0 `Backends::new_transaction_builder` returns the boxed
  builder directly rather than a `TransactionBuilderParts`, so the registry layer
  carries no router state. Router customisation, if we need it, happens higher up.

### Revisit if

We hit a capability that can only be reached from inside the upstream tree. The
conformance suite (`surrealdb-kvs-test`) is `publish = false`, so vendoring that
one crate is already an accepted exception — see ADR-0005.

---

## ADR-0004 — Verify layout assumptions against published artifacts, not the default branch

**Status:** accepted · **Date:** 2026-10-01

### Context

ADR-0001 exists because we got this wrong once. The mistake was cheap to make
and would have been very expensive to build on.

### Decision

Before designing against any upstream claim, corroborate it against an artifact
a consumer can actually see:

- Does the crate exist on crates.io, at what version, under what licence?
- Does the published API match what the source tree suggests?

A source-tree fact is a hypothesis. A published-artifact fact is evidence.

### Consequences

`make audit-upstream` regenerates the crate inventory and licence table into
`docs/upstream-crates.md`, so this check is repeatable and cheap.

---

## ADR-0005 — Use the upstream conformance suite

**Status:** accepted · **Decided:** 2026-10-01 · **Implemented:** 2026-10-02

### Context

`surrealdb-kvs-test` is a shared contract suite for KV backends, in the upstream
tree, with `publish = false` — so it is not on crates.io. Its own docs say every
backend "first-party or external" runs the same tests.

### Decision

Vendor that single crate and make our engine pass it. Its modules —
`builder_surface`, `defaults`, `edges`, `lifecycle`, `multi`, `raw`,
`savepoint`, `snapshot`, `timestamp`, `versioned` — become our acceptance
criteria, and passing them becomes our definition of "behaves like a real
backend".

The vendored code is **BUSL-1.1** (SurrealDB's licence, like every crate we
link), not Apache-2.0 — so is anything that links it, including our test target.
`vendor/surrealdb-kvs-test/VENDOR.md` records the ref and the provenance;
`NOTICE` records the licensing path.

### Outcome

Vendored at `vendor/surrealdb-kvs-test/`: upstream `surrealdb/kvs-test` at tag
`v3.3.0` (`238bfeb`), `src/` byte-for-byte, with its `LICENSE` alongside. It cost
four dependencies — `inventory`, `surrealdb-kvs`, `libtest-mimic`, `tokio` — and
dragged in nothing else, so the question of whether vendoring it alone was cheap
is settled: it was. `cargo test -p surrealdb-ds --test kvs` runs it.

The `upstream/` tree stays a reference and never a build input: cargo sees the
copy under `vendor/`, not the checkout.

See ADR-0006 for how the engine is registered against it, and PROGRESS.md for the
first run and what it took.

### Consequences

- Our test target is BUSL-1.1, and is marked as such.
- Vendoring one crate is a much smaller exception than forking the repo.
- The suite's *skips* are part of the contract too. What it declines to check for
  us is a gap we have to close ourselves, and saying so is cheaper than letting a
  green run imply coverage — see ADR-0006.
- We should still write our own differential tests on top; the upstream suite
  tests the contract, not parity with SurrealDB specifically.

### Revisit if

Upstream publishes the suite, or if it turns out to encode SurrealDS-specific
behaviour we should be deriving independently instead.

---

## ADR-0006 — Register the engine as `surrealds` in the conformance suite

**Status:** accepted · **Date:** 2026-10-02

### Context

The suite does not have one contract; it has one contract **per backend name**.
Each test declares `only = [...]` / `except = [...]` against the name a consumer
registers, and the names are an open vocabulary, so a test can reference a
backend that lives in another repository.

That vocabulary already contains **`surrealds`** — the distributed store this
project reimplements — in nine places, including as the reason two tests exist at
all: `raw::getu_unsupported` (`only = [surrealds]`) and
`multi::multiwriter_same_keys_allow` (`only = [tikv, indxdb, surrealds]`).

So the suite's shape forces a choice to be made explicitly: register as
`surrealds` and answer to what the real distributed store is documented to do, or
register under our own name and answer to whatever is easiest.

### Decision

Register as **`surrealds`**.

### What that costs, and why it is worth it

Registering as `surrealds` is strictly *harder*, and it changes the transaction
model rather than just the test count:

| | `surrealds` | any other name |
| --- | --- | --- |
| Overlapping blind writes to one key | **all commit, last wins** | first committer wins, the rest refused |
| Write skew | must be **prevented** | permitted, and asserted to be |
| `getu` | must be **refused** | must work; four conflict tests run |
| `register_metrics` | **required**, every declared name collectable | must return `None` |
| `transactions_local` | must report **local** | must report local |

The first row is the one that matters. First-committer-wins is a *different
engine* from the one being reimplemented: the suite describes the distributed
store's model as write-set validation that retries a blind write at a higher
timestamp, so overlapping blind writes serialise in timestamp order instead of
aborting. Under any other name we would have rejected same-key writers and called
it conformance.

It also makes the second row honest. The suite *ignores*
`write_skew_permitted` for `surrealds` on the grounds that we are serializable —
which leaves nothing in the conformance run holding us to it. That gap is ours to
close, and `crates/surrealdb-ds/tests/serializable.rs` closes it: write skew is
refused, the refusal is retryable, a refused transaction can be re-executed and
then commit, disjoint writes both commit, a read-only commit never conflicts, and
a concurrent write inside a *scanned range* counts as a conflict.

### Consequences

- The engine's transaction model is snapshot isolation with **read-set validation
  at commit**, and blind writes never conflict. Explicitly not first-committer-wins.
- `getu` stays refused with `UnsupportedLockedReads`, and the four `getu_*`
  conflict tests stay reported *ignored*. A real, unfilled gap: plain reads get
  commit-time validation and `SELECT … FOR UPDATE` inherits it, which is stronger
  than a row lock but is not the same code path.
- Every green run has to be read next to the list of what the suite ignored. It
  is written out in PROGRESS.md rather than left to whoever reads the output.
- The name is a claim. If our behaviour diverges from the real distributed store's,
  the suite keeps passing under this name — which makes the Phase 2 differential
  harness more important, not less.

### Revisit if

Upstream renames or drops `surrealds` from the vocabulary, or if we implement
locked reads — at which point `getu` moves from refused to required and the
`surrealds` skips become failures we have to satisfy.

---

## ADR-0007 — L2 is byte fidelity at the KV boundary, and the harness replays bytes rather than regenerating them

**Status:** accepted · **Date:** 2026-10-04 · **Next free: ADR-0008**

### Context

Phase 1 opened with a task inherited from the original plan: *reconstruct the key
encoding contract from the published interface*. Its premise was that
`surrealdb-kvs`'s `key` and `value` modules — named in the plan as the whole of
what L2 means — spell out how a key is encoded.

They do not. `surrealdb-kvs` `src/lib.rs` says so itself: *"This crate defines
how a key and a value are **declared**; it does not declare any. Which keys exist
is the keyspace, which lives above it."* `KVKey::encode_buffer` and
`KVValue::kv_encode_value` are traits; the layout is declared one level up by a
single `keyspace!` invocation in `surrealdb-datastore`, with the encoders
generated by `surrealdb-keyspace-macro` (R-0044). All three are published
crates, and all three were already in our lock file before this decision.

Two further facts settled the shape of the work:

- **`Transactable` is byte-level** (R-0045). A key *is* a `Cow<[u8]>`; the
  backend never decodes. The query layer hands a backend already-encoded bytes,
  so byte compatibility is **faithful passthrough**, not a re-implementation of
  an encoder. Writing our own encoder would have been strictly worse — more
  code, more risk, and a busier reading of the clean-room line for no gain.
- **The oracle was already in our binary.** `surrealdb-kvs-rocksdb` and
  `surrealdb-datastore` were in `Cargo.lock` transitively via `surrealdb-server`,
  so `construct-only rocksdb:<path>` runs upstream's real engine and writes
  upstream's real files. A "dataset written by upstream" cost nothing to obtain.

### The decision that was actually load-bearing: replay, don't regenerate

The harness was first designed to *generate* a dataset on both tiers and compare.
Measured: the same SurrealQL workload run twice against upstream's own `rocksdb:`
produces 55 keys of which only **35** are identical. The other 20 mint a fresh
`Uuid::new_v7()` per run. A regenerated comparison is therefore a flaky
comparison, and masking the unstable bytes is worse: `Uuid::new_v7()` embeds a
millisecond timestamp, so two runs share its high bytes and any pinned subset
goes stale within minutes.

So the harness **copies** instead. Upstream authors once via real SurrealQL →
the whole keyspace is dumped → **those exact bytes are replayed** into
`ds+mem://` and read back → a third store replays our output back into upstream's
engine as a control. Every key and every value is compared, with no exclusions,
because the bytes compared were copied rather than independently regenerated.

### Consequences

- **PLAN.md's Phase 1 task 1 is deleted, not attempted.** There is nothing to
  reconstruct. What Phase 1 lacked was not knowledge of the encoding but
  *evidence* that the bytes we hold are the bytes a real node holds.
- **`make golden` is the instrument**, in `make test` and named as its own CI
  step. The step is deliberate duplication — `make test --workspace` already runs
  the tests — because their output is captured by default and this is the
  project's definition of 1:1; the evidence should be legible in the run list,
  not hidden inside a test count.
- **A committed manifest pins 35 of the 55 keys**, carrying 1060 of 1526 value
  bytes. The other 20 are not in the file because they carry a per-run UUID —
  `!tb{tb}`, which is where `PERMISSIONS` lives, among them. Their bytes are
  still compared absolutely by the round trip. A pin that covers a third of the
  dataset and says so is worth more than a whole-of-dataset pin that goes stale.
- **The harness is falsifiable, which was verified rather than asserted.** A
  single flipped bit injected into `DsTxn::set` for 172-byte values fails two of
  the three golden tests, at the injected offset, with the injected mask. It was
  tried twice and reverted; the engine crate's diff is empty.
- **ADR-0002's link table was incomplete.** R-0036, R-0037, R-0038 and R-0040
  had cited `surrealdb-core` as `PUBAPI` since 2026-10-02, a day after the table
  was written. `surrealdb-core` and `surrealdb-keyspace-macro` are now listed.

### What L2 now means, and what it still does not

L2 is **byte fidelity at the KV boundary**: the bytes our tier stores and returns
for a given logical dataset are the bytes a real SurrealDB tier stores and
returns. That is now evidenced over 55 keys, including graph edges, a unique
index, an HNSW vector index with its serialised vectors, record documents,
definitions, and a tombstone, with version lists and tombstones compared across
nine steps including a reader pinned at v1.

Three limits are load-bearing and are recorded in `PROVENANCE.md` rather than
left to the reader:

1. **No key class is proven complete.** Only classes this workload reaches. A
   class upstream writes that it never touches is untested, and no failure would
   announce it.
2. **On-disk format compatibility is not addressed.** Our tier is in-memory. That
   a plain `rocksdb:` path installs *no* timestamp comparator at all —
   `RocksDbConfig::versioned` defaults to `false`, verified in the `OPTIONS`
   file as `comparator=leveldb.BytewiseComparator` (R-0048) — means the on-disk
   question is open in both directions, and the UDT layout applies only behind
   `?datastore_versioned=true`.
3. **Our refusal of versioned reads is upstream's own position, not a gap.**
   `surrealdb-kvs-mem` rejects `datastore_versioned` at startup with
   `UnsupportedVersionedQueries` (R-0049). This reframes PLAN.md's versioned-read
   task: implementing time-travel would make our tier *diverge* from upstream's
   local tier, so it is a design decision rather than a conformance gap.

### Revisit if

- A key class outside this workload fails to round-trip — the manifest grows a
  second dataset rather than this one growing a `PERMISSIONS` workaround.
- The local durable backend is built, at which point the on-disk question becomes
  answerable and belongs in its own ADR.
- Upstream stabilises the datastore keyspace, or ships a format we must match, or
  changes `Transactable` away from byte-level keys.

---

## ADR-0008 — The durable tier targets full on-disk interchangeability with upstream

**Status:** accepted · **Date:** 2026-10-04 · **Next free: ADR-0009**

### Context

ADR-0007 established that L2 is currently evidenced only at the KV boundary: our
tier preserves the bytes it is handed, proved over 55 keys. It deliberately left
the on-disk question open, because "byte compatible" admits two very different
readings and they differ by roughly two orders of magnitude in work:

| | Meaning | Cost |
| --- | --- | --- |
| **KV-boundary** | The `(Key, Val, Version)` stream we read and write matches upstream's | Already evidenced (ADR-0007) |
| **On-disk** | A directory we wrote can be opened by a stock `surreal`, and vice versa | Cloning the reference tier's format |

The owner chose **full on-disk interchangeability**. Recorded here with its cost
stated plainly, because this record's value is that a later reader can tell what
was chosen *and* what it cost.

### Decision

Our local durable tier writes a database that upstream's `surrealdb-kvs-rocksdb`
can open, and can open one upstream wrote. `rocksdb` is **our** dependency;
`surrealdb-kvs-rocksdb` is not linked as our implementation (ADR-0002, PLAN.md).

**Target profile: the unversioned default.** A stock `surreal` opens
`rocksdb:<path>` with no query string, so that is the profile our files must
satisfy. It is also the profile matching our current semantics — we refuse
versioned reads, as upstream's own memory backend does (R-0049). The versioned
profile, with its `surrealdb.TimestampComparator` and 8-byte little-endian
timestamps, is **not** this target; it is deferred and belongs with whichever
decision implements time-travel.

### The ground truth, captured

Produced by our own binary — `construct-only rocksdb:<tmp>` — and kept at
`docs/evidence/upstream-rocksdb-OPTIONS-3.3.0.txt`. RocksDB 11.0.0,
`options_file_version=1.1`, one column family `default`. The settings that are
format-relevant rather than merely tunable:

| Setting | Value | Why it matters |
| --- | --- | --- |
| `prefix_extractor` | `surrealdb.TablePrefix.v1` | **A named extractor.** RocksDB resolves it by name on open; an absent name is an open failure, not a warning |
| `comparator` | `leveldb.BytewiseComparator` | Confirms R-0048 — no UDT comparator on the default profile |
| `enable_blob_files` | `true` | Large values live in separate blob files, not the SSTs |
| `format_version` / `checksum` | `7` / `kXXH3` | Must be written and read identically |
| `index_type` / `partition_filters` | `kTwoLevelIndexSearch` / `true` | Follows from the prefix extractor |
| `compression` / `bottommost_compression` | `kSnappyCompression` / `kZSTD` | |
| `write_buffer_size` / `max_write_buffer_number` | 128 MiB / `8` | |
| `target_file_size_base` / `max_bytes_for_level_base` | 64 MiB / 256 MiB | |

A dated capture is evidence and will drift. `surrealdb-kvs`'s own warning that
its API may break "even between patch versions" applies to this file too, so it
is regenerated rather than trusted.

### The honest cost

This is the option that walks closest to the clean-room line, and the tension
should be named rather than smoothed over.

Under ADR-0002 we link SurrealDB's BUSL-1.1 crates and that encumbrance is
already paid. Reading their source for *interface shape* is permitted. Matching an
**on-disk format** is a stronger claim than reading an interface: the format's
only complete specification is upstream's implementation, and
`surrealdb.TablePrefix.v1` is a name we must reproduce for RocksDB to accept the
file at all. So this decision trades some clean-room comfort for an operational
property — a stock binary can read our disk — and the trade is deliberate.

Two things keep it defensible. Everything here stays inside one BUSL-encumbered
crate, which the Apache-2.0 grant never covered anyway (NOTICE, path 2). And
`prefix_extractor.rs` tells us what a prefix *means*; the encoding is RocksDB's,
not SurrealDB's, and is documented by RocksDB itself.

### Sequencing

De-risk before building, as ADR-0003 did for the seam. The first tranche is
therefore not the tier — it is the smallest experiment that can **falsify** this
decision: open an upstream-constructed directory with `rocksdb` as our own
dependency and read the keyspace back byte-identically. If a named prefix
extractor we cannot satisfy, or blob-file layout, makes that impossible, we learn
it in a day rather than after several thousand lines.

### Revisit if

- Opening an upstream directory proves impossible without linking their crate, in
  which case the operational property is unavailable and this ADR is superseded
  by one recording why.
- `surrealdb.TablePrefix.v1` changes shape across releases, making byte-level
  interchange a moving target rather than a fixed contract.
- Time-travel reads are implemented, which promotes the versioned profile from
  deferred to required and needs its own capture.

---

## ADR-0009 — Build on a Fly machine; it is currently blocked on a glibc base

**Status:** accepted, with the final step outstanding · **Date:** 2026-10-04

### Context

The workstation's disk is at capacity: 898G with 18G free, of which
`target/debug` alone is 32G — `deps` 22G, `build` 8.5G, `incremental` 1.7G, and
a 1.8G debug `surrealdb-ds-server`. A build cannot fit its own artifacts in the
remaining space, so local building had stopped being an option rather than
merely being slow.

`surrealdb-kvs-rocksdb` and `surrealdb-datastore` were already in `Cargo.lock`
via `surrealdb-server`, so RocksDB 11.0.0 was **already compiled into our
binary**. That made a Fly builder cheap: same lockfile, same compiler, and the
oracle we needed for ADR-0007.

### Decision

Compilation happens on a Fly machine reached over **2PN** (wireguard), so there
is no public SSH port. `cargo remote-3000` rsyncs the project, runs cargo over
ssh, and copies artifacts back **only** under `--copy-back`. `make test` and
`make clippy` therefore return output and nothing else, which is what makes the
trade worth making.

Three things were needed to make that work, and each cost a round trip:

1. **Our own OpenSSH server on port 2222.** Fly's `Hallpass` on 22 argv-splits
   the command instead of running it through a login shell, and reads no
   `authorized_keys` from disk. cargo-remote needs a shell twice: rsync's
   `--rsync-path` is `mkdir -p DIR && rsync`, and its build script is
   `cd DIR; . env; cargo ...`. A normal `sshd` also authenticates the plain
   ed25519 key, so nothing depends on an expiring certificate or the agent.
2. **`host` is an ssh_config alias.** cargo-remote splits its `host` field on
   `:` to separate user from hostname, which destroys a bare IPv6 literal.
   Its `ssh_port` field overrides `Port` in `ssh_config`, so the two must agree.
3. **`$CARGO_HOME/env` written by hand.** `rustup --no-modify-path` does not
   create it, and cargo-remote sources it; without it every remote build dies
   with `ash: cargo: not found`.

`ops/fly-builder/provision.sh` captures all of this and is idempotent, because
the machine's root filesystem is ephemeral and `apk` packages do not survive a
stop. `/data/bin/ensure-ready` exists for the same reason: `flyctl machine exec`
argv-splits rather than using a shell, so a one-word executable on the volume is
the only way to bootstrap in one call.

### What works, and what does not

Transfer, execution and the toolchain are verified: `cargo remote-3000 -r fly
locate-project` returns the remote path, cargo/rustc/clippy are 1.95.0 — the
version `ci.yml` pins — and most of the graph compiles remotely, C++ crates
included.

**The build does not complete, and the reason is not a missing package.**
`rquickjs-sys`'s build script panics with `Unable to find libclang: ...
Dynamic loading not supported`. bindgen is a build-dependency of both
`surrealdb-librocksdb-sys` and `rquickjs-sys`, but only the former enables
bindgen's `runtime` feature, and resolver `"3"` keeps the two feature sets
separate — so `rquickjs-sys` gets a bindgen that must **link** libclang at build
time rather than dlopen it. That is fine on glibc, which is what this
workstation and upstream's CI images are, and impossible on musl. No package can
fix it.

### The outstanding step

Building on glibc, Debian for parity with CI. It could not be done
non-destructively:

- `flyctl machine update --image debian:bookworm-slim` → *"deploying over the
  remote builder is not allowed"*. The app is a registered org build machine and
  Fly will not overwrite its own builder's image.
- `flyctl machine create … -v builder_data:/data` → *"remote builders may have
  only one volume"*, and the existing machine holds it.

So the Debian machine requires **destroying the current one**, which also
discards `/data` and forces a rustup reinstall. That is an owner's decision
because it is destructive and it costs machine-time, so it is not done here.
`ops/fly-builder/fly.glibc.toml` is the config for it.

### Consequences

- **`docs/remote-builds.md` is the operational reference**, and `AGENTS.md`
  tells agents to read it before assuming remote builds work — and to check
  whether a failure is the musl/libclang wall or something new.
- **`make golden` has not been run remotely.** The golden harness links the
  server binary, which is the heaviest thing we build; it is the first thing to
  try once the base is glibc.
- **Do not `cargo clean` locally yet.** Local building is the only working
  builder, and reclaiming that 32G before the remote one can compile would leave
  no way to build at all.
- The honest summary is that this ADR buys a working *transport* and a proven
  *diagnosis*, not yet a working *build*.

### Revisit if

- The Debian machine is created, at which point this becomes a routine
  infrastructure note and the "outstanding step" section is deleted.
- The volume limit on remote builders is lifted, which would allow a second
  machine alongside the Alpine one and remove the need to destroy anything.
- Local disk is reclaimed some other way, which removes the pressure that made
  this urgent.

---

## ADR-0010 — A shared, multi-project Debian build server

**Status:** accepted · **Date:** 2026-10-04 · **Next free: ADR-0011

Supersedes the deployment half of ADR-0009, which reached the right diagnosis
(builds must be offloaded) by the wrong route (an Alpine box that could not
build the workspace). ADR-0009's musl finding stands and is why the base changed.

### Context

ADR-0009 got the transport working against a Fly machine but could not finish a
build, because musl cannot. It also could not fix that in place: the app was a
registered org build machine, so Fly refused both `machine update --image`
(*"deploying over the remote builder is not allowed"*) and a second machine
(*"remote builders may have only one volume"*). Both limits are **app-scoped
privileges of being a builder**, so the fix was a separate app rather than a
fight with the builder one.

The workstation still could not build: `target/debug` is 32G against 18G free.

### Decision

A dedicated app, `open-surrealdb-ds-builder`, running `debian:bookworm-slim` on
`performance-8x` (8 dedicated cores, 16 GB) with a 100 GB volume at `/data`,
reached over **2PN** only. Debian for parity with upstream's CI images, and
because glibc is a hard requirement rather than a preference.

Dedicated cores rather than shared: build throughput has to be predictable, and
`shared-cpu-8x` is noisy-neighbour throttled. It is also not the constraint —
8 dedicated cores for a few minutes a day is not an expensive thing to own.

**It is a shared server, not a personal builder.** Several projects build on it
at once, so:

- `/data/cargo` and `/data/rustup` are shared, so the crates.io cache and the
  toolchain are paid for once across every project.
- `/data/builds/<hash>` is per project, named by cargo-remote after a hash of the
  project path, so projects cannot collide and agree on no naming scheme.
- Compiled artifacts are *not* shared; they stay in each project's `target/`.
  Sharing them would want sccache, which is a later decision.
- One build gets `[build] jobs = 4` of 8 cores, so two concurrent projects divide
  the machine instead of starving. Overridable per invocation with
  `-b CARGO_BUILD_JOBS=N`.
- No project-specific state lives on the box. A project contributes a
  `Makefile` target and nothing else, so a new project needs no provisioning.

The operational reference is the `shared-flyio-build-server` agent skill. The
machine definitions and provision scripts live in that skill rather than in this
repository: they describe a shared environment, not this project, and every
project would otherwise carry a copy that drifts.

### Consequences

- **`-d 1.95.0` is mandatory and has no config-file equivalent.** cargo-remote's
  toolchain flag defaults to `stable` and it runs `rustup default` on every
  build, so omitting it silently installs and switches compiler versions. A rustc
  bump must not be mistakable for a contract change, which is why `ci.yml` pins
  1.95.0 too.
- **`env` in a cargo-remote config is a list of files to source, not
  `KEY=VALUE`.** Setting it to a variable name displaces the default
  `~/.cargo/env` and every build fails with `cargo: command not found`. Both that
  and the toolchain flag cost a debugging round and are in the skill.
- **`debug = 0` remotely.** The same build is 32G locally with debuginfo and
  about 3.5G on the box. Nothing is debugged on a build server.
- **Our whole tree fits with room to spare**, so concurrency is a CPU question
  rather than a disk one.
- **The lockfile is shared, so resolution is identical.** `sha256sum` of the
  remote `Cargo.lock` matches the local file, which is the check that matters:
  what compiles must not depend on where it compiles.

### Revisit if

- A second region or app becomes necessary, at which point per-region pinning
  matters because 2PN is low-latency only within a region.
- Build times become painful enough to justify sccache, which would share
  compiled artifacts across projects as well as sources.
- The disk pressure that motivated this is relieved some other way, which would
  remove the reason to keep a build box running at all.

---

## ADR-0011 — On-disk interop works; the prefix extractor fails silently, not loudly

**Status:** accepted · **Date:** 2026-10-04 · **Next free: ADR-0012

Corrects two claims in ADR-0008. Both were mine, both were checked before being
written down, and both were wrong.

### What ADR-0008 got right

The read half of the decision holds. `tests/interop.rs` opens a directory
upstream's own engine wrote, using `rocksdb` as *our* dependency, and reads back
**55 keys / 1526 value bytes byte-identically** — every key and every value, no
exclusions. `make interop`. So a stock upstream directory is readable by our
code, which is the property ADR-0008 was reaching for.

### Correction 1 — a missing extractor name is not an open failure

ADR-0008 says of `prefix_extractor=surrealdb.TablePrefix.v1`: *"RocksDB resolves
it by name when opening. An extractor that is not registered under that exact
name is an **open failure**, not a warning."*

**It is not.** `DB::open` never reads the on-disk `OPTIONS` file at all. The
directory opens with no extractor registered, and equally with one registered
under a deliberately wrong name. Both of those are asserted as tests, precisely
because the premise was load-bearing and false (R-0051).

The name in `OPTIONS` is inert. It does not have to be reproduced for the open to
succeed.

### Correction 2 — the real failure mode is worse than an error

A wrong extractor does not fail. It **silently widens** the read. A
prefix-restricted seek returns the entire run instead of the narrowed range, and
the caller cannot tell:

```
2 rows with our extractor, 24 with a mismatched one, 24 with none — wider, not an error
```

This is the exact hazard class this project's other risks live in: a subtly wrong
rule producing **silently wrong data rather than an error**. It is worse than the
open failure ADR-0008 predicted, because it will not announce itself.

### Correction 3 — byte equality does not validate the extractor

A methodological finding worth more than either correction. Injecting an
off-by-one into our extractor left the headline byte-equality test **green**.
Reading a whole keyspace and comparing it byte-for-byte cannot tell a correct
prefix extractor from a slightly-short one, because a too-short prefix reads
*more* rows and the comparison still matches. The extractor therefore needs its
own assertions about narrowed ranges, which is what
`a_mismatched_extractor_silently_widens_a_prefix_restricted_read` exists for.

### What `TablePrefix.v1` actually is

Variable-length, ending at and including a one-byte discriminator. In domain iff
the key is ≥14 bytes, `key[0,1,6,11]` are `/ * * *`, there is a `\0` at offset
≥12, and at least one byte follows it. The prefix is `key[..null_pos + 2]`.
`*` records, `+` index entries, `!` metadata, `~` edges, `&` references. Keys
outside the domain are returned unchanged and excluded by `InDomain`. 39 of our
55 keys are in domain. Source: `surrealdb-kvs-rocksdb` `src/prefix_extractor.rs`
(`TB_START`, `MIN_LEN`, `parse_prefix_end`) — `SRC`, format shape,
reimplemented from the documented layout.

### The binding was a cost decision, not a necessity

ADR-0010 implied the fork was required. It is not. `format_version = 7` does not
discriminate: `kLatestBbtFormatVersion` is 7 in RocksDB 11.0.0, 11.8.1 and 10.4.2,
with a read floor of 2 in all three. The public `rocksdb` crate could read these
files.

We use `surrealdb-rocksdb` 0.24.0-surreal.5 (wrapping RocksDB 11.0.0) because it
is **already in our `Cargo.lock`**, so it cost one edge in the lockfile and zero
rebuild, against a full C++ rebuild of a different RocksDB version. That is a
cost judgement and it is recorded as one, with the alternative named. Features
are pinned to `lz4` and `snappy` because taking defaults adds zstd, zlib,
bzip2 and bindgen and forces that recompile.

It is a SurrealDB-maintained fork, so `NOTICE` gained a fourth licensing path
rather than letting a crate that is neither vendored nor ours-by-name pass
unremarked.

### What is still unproven

- **Writing.** ADR-0008's other half: a directory *we* wrote, opened by upstream.
  Nothing has been written in our own format yet.
- **Key-class completeness**, as ever.
- **Anything past one L0 SST.** Compression, blob files, two-level indexes and
  partitioned filters are all configured in the `OPTIONS` file and none has been
  exercised. `bottommost_compression = kZSTD` in particular is never reached,
  because upstream links only lz4 and snappy — the `OPTIONS` file records
  defaults, not behaviour.
- **Bloom-filter correctness at scale**, and the Rust API's stability.

One upstream behaviour worth recording because it cost a debugging round and
would have looked like corruption: **a clean shutdown rewrites its own `/!nd`
row** (R-0053). Comparing a pre-shutdown dump against a closed directory fails on
that byte, reproducibly.

### Revisit if

- A durable tier writes its own database and upstream opens it — the write half,
  and the only thing that would settle ADR-0008 completely.
- The public `rocksdb` crate becomes preferable, which the version data says it
  already is on capability grounds.
- A key class outside this workload fails to round-trip, which would mean the
  keyspace is not the closed set `keyspace.map` claims.
