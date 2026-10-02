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

**Status:** **open — needs an owner decision** · **Date:** 2026-10-01

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

### Decision

**Deferred.** The current default for the scaffold is **Path A**, because it is
the leverage-optimal starting point and it is reversible: Path A's engine logic
lives in engine-specific modules with no SurrealDB imports at the algorithmic
layer, so a later move to Path B is a matter of reimplementing the boundary
types, not the consensus and storage code.

But this is an owner's call, not a technical one, and it depends on intent we do
not know: research artifact, internal infrastructure, or a product?

### Consequences either way

- `LICENSE` (Apache-2.0) covers **only** code authored here.
- Every crate that links SurrealDB crates must declare `license = "BUSL-1.1"` in
  its `Cargo.toml`. We do this today; it is not cosmetic.
- The two paths must not be blended inside a single crate.

### Revisit if

The project acquires a commercial goal, or if we decide the upstream
conformance suite is worth less than a clean licence. Also revisit after
**2030-01-01**, when BUSL converts to Apache-2.0 and Path A's cost largely
disappears.

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
   `register(DsBackend::new())`, and constructs through our scheme. Remaining
   question is whether `surrealdb-server` exposes an init path that accepts a
   caller-built registry, or builds its own internally.
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

**Status:** accepted · **Date:** 2026-10-01

### Context

`surrealdb-kvs-test` is a shared contract suite for KV backends, in the upstream
tree, with `publish = false` — so it is not on crates.io. Its own docs say every
backend "first-party or external" runs the same tests.

### Decision

Vendor that single crate (Apache-2.0 era source, BUSL-1.1 terms) and make our
engine pass it. Its modules — `builder_surface`, `defaults`, `edges`,
`lifecycle`, `multi`, `raw`, `savepoint`, `snapshot`, `timestamp`, `versioned`
— become our acceptance criteria, and passing them becomes our definition of
"behaves like a real backend".

### Consequences

- Our test target is BUSL-1.1, and is marked as such.
- Vendoring one crate is a much smaller exception than forking the repo.
- We should still write our own differential tests on top; the upstream suite
  tests the contract, not parity with SurrealDB specifically.

### Revisit if

Upstream publishes the suite, or if it turns out to encode SurrealDS-specific
behaviour we should be deriving independently instead.