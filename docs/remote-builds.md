# Remote builds

Compilation happens on a shared Fly machine rather than locally, because the
workstation's disk is at capacity. This repo's `target/debug` was 32G (`deps` 22G,
`build` 8.5G, `incremental` 1.7G, plus a 1.8G debug `surrealdb-ds-server`) and has
been deleted; `/home` has since been observed at 100% with 4.2G free, so local
building is unavailable regardless.

The operational reference — including how to add another project — is the
**`shared-flyio-build-server` agent skill**. This file records what is specific
to *this* repository: what the commands are, and what the server does and does
not change for us.

## Commands

Every `make` target takes a `CARGO` override, so the documented targets work
verbatim once it points at the remote:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd /home/bitnom/Code/open-surrealdb-ds

CARGO='cargo remote-3000 -r fly -d 1.95.0 --'

make check      CARGO="$CARGO"
make test       CARGO="$CARGO"
make golden     CARGO="$CARGO"
make interop    CARGO="$CARGO"
make durability CARGO="$CARGO"
make remote CMD='clippy --workspace --all-targets -- --deny warnings'
make remote-stop
```

Two things in that string are not optional: `-d 1.95.0`, because the toolchain
default is `stable` and every build runs `rustup default`; and the `--`, because
cargo-remote parses its own flags after the subcommand too and its `-p` is
`--ssh-port`. See the skill's Traps.

`check`, `test` and `clippy` return output and nothing else, because cargo-remote
copies artifacts back only under `--copy-back`. To get the server binary for
`make smoke`:

```bash
cargo remote-3000 -r fly -d 1.95.0 -- build -c=debug/surrealdb-ds-server -p surrealdb-ds-server
```

Without `CARGO`, the targets call local `cargo` — which is what CI runs, and which
**cannot complete on this workstation** while `/home` is at 100%.

## The server

| | |
| --- | --- |
| App / machine | `open-surrealdb-ds-builder` / `87477e0cd01748` |
| Spec | `performance-8x`, 8 dedicated cores, 16 GB |
| Volume | 100 GB at `/data` |
| Base | `debian:bookworm-slim`, glibc |
| ssh | `fly-builder` on port 2222, over Fly 2PN |

There is no public port of any kind. `CARGO_HOME` and `RUSTUP_HOME` are on the
volume, so the toolchain and the crates.io cache survive a restart and are shared
with any other project on the box.

## What the server changes for us

Three things, all of them deliberate:

- **`rustc` is pinned to 1.95.0**, the version `ci.yml` verifies. A rustc bump
  must not be mistakable for a contract change, so `-d 1.95.0` is passed
  explicitly rather than left to float.
- **Debug info is off** (`[profile.dev] debug = 0`, `incremental = false`). A
  debug build of SurrealDB is ~32G locally with debuginfo, and nothing is
  debugged on a build server. Our whole tree lands in about 3.5G.
- **One build gets `[build] jobs = 4`** of 8 cores, so two projects can build at
  once without starving each other.

## What it does not change

- **The lockfile and feature resolution are identical.** The box uses our
  `Cargo.lock`; `sha256sum` matches the local file. Nothing about what compiles
  is affected by where it compiles.
- **glibc, like local and CI.** This was the whole reason for moving. On Alpine
  the workspace *cannot* be built: `rquickjs-sys`'s build script panics with
  `Unable to find libclang: ... Dynamic loading not supported`. bindgen is a
  build-dependency of both `surrealdb-librocksdb-sys` and `rquickjs-sys`, but
  only the former enables bindgen's `runtime` feature, and resolver `"3"` keeps
  the two feature sets separate — so `rquickjs-sys` gets a bindgen that must
  **link** libclang at build time rather than dlopen it. Fine on glibc,
  impossible on musl. See ADR-0009 for the full Alpine attempt.
- **The vendor tree is still needed.** `vendor/surrealdb-kvs-test/` is tracked
  and transfers with the sources, so the remote build needs no network step for
  it.

## Cost and lifecycle

The machine bills while it runs. Start it before a remote build, stop it when
done:

```bash
flyctl machine start 87477e0cd01748 -a open-surrealdb-ds-builder
flyctl machine stop  87477e0cd01748 -a open-surrealdb-ds-builder
```

Its root filesystem is ephemeral, so after any restart the packages must be
reinstalled — see the skill for the two-step bootstrap. `/data` persists.

## Timing

A first build on a fresh volume is 20–40 minutes, dominated by
`librocksdb-sys`'s C++ compile. Later builds reuse `/data/builds/<hash>/target`
and take minutes. Run long builds detached and poll the log rather than holding
a terminal open.