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
