# `docs/spec/` — the behavioural specification

Currently **empty**, and that is a deliberate state rather than an oversight.

Every requirement this project implements gets a specification here: a testable
statement of the behaviour, with the provenance record that establishes how we
know it. Nothing lands here without one. See [`../../PROVENANCE.md`](../../PROVENANCE.md)
for the log and the rule.

## Why it is empty

The specification is written **per phase, from the evidence that phase produced** —
not ahead of it. Two reasons, and the second is the important one:

1. **There is nothing to specify yet that we have not already built.** Phases 0–1
   are implementation against a *published interface*: `surrealdb-kvs`'s traits
   are the specification, and `vendor/surrealdb-kvs-test` is the executable form of
   it. Writing prose that restates them would add a document that can only drift
   from the code it describes.

2. **The behaviours worth specifying are the ones we cannot read.** A requirement
   sourced from `PUBAPI` needs no specification — the interface *is* one. A
   requirement sourced from `OBS` (black-box measurement of a running instance)
   or from `REL` (a release note describing a failure mode) does: there is no
   interface to read, only an observed behaviour to pin down. `PROVENANCE.md`
   tracks exactly those as open **Gaps**, and every one of them lands in Phase 2
   or later — conflict-detection windows, `safe_timestamp` under partial
   visibility, read-your-writes across nodes, clock skew.

## What will land here

One file per requirement cluster, in `R-NNNN` order, each opening with its
provenance record:

```
docs/spec/R-00NN-<slug>.md
```

Format, per `PROVENANCE.md`:

1. **Requirement** — stated as a testable assertion, not a paraphrase.
2. **Source** — class (`DOC` / `REL` / `OBS` / `PUBAPI` / `SRC`), citation, access
   date. `OBS` additionally carries the reproduction: instance type, version,
   commands run, observed output.
3. **Behaviour** — what happens, including the failure modes a caller must
   handle.
4. **How it is checked** — the test that would fail if the behaviour regressed,
   and where it lives. A requirement with no test and no reason one cannot exist
   is not finished.

## The rule

> If a requirement's source is ever unclear, it does not go in the spec. Finding
> out *how you know* is the entire point.

That rule is why this directory can be empty for months and still be honest: it
is empty because nothing has been *asserted* yet, not because nothing is known.