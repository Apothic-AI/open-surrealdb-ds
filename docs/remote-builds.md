# Remote builds

The local machine's disk is at capacity (898G filesystem, 18G free, with
`target/debug` alone at 32G), so compilation is meant to happen on a Fly machine
and only test output and small binaries come home.

**Status: transport works, the build does not finish.** Read this before
assuming remote builds are available. Details and evidence below.

---

## How it is wired

```
local                                   Fly builder (2PN, private only)
─────                                   ────────────────────────────
cargo remote-3000 -r fly <cmd>
  ├─ rsync sources ──────────────────▶  /data/builds/<hash>/
  ├─ ssh → login shell ─────────────▶  . ~/.cargo/env; cd DIR; cargo <cmd>
  └─ (nothing comes back)             ◀─ exit status + build output on stderr
```

Both ends are `Box<dyn Transactable>`-adjacent in spirit: the remote is just a
place cargo runs. `cargo remote-3000` transfers the project, runs cargo over
ssh, and copies artifacts back **only if `--copy-back` is passed**. So
`make test` and `make clippy` return nothing but output, which is what makes
this worth doing.

## Configuration

Three pieces, none of which are in the repository because two of them are
machine-specific:

| Where | What |
| --- | --- |
| `~/.ssh/config`, `Host fly-builder` | address, port, identity |
| `~/.config/cargo-remote-3000/cargo-remote-3000.toml` | `[[remote]] name = "fly"` |
| `ops/fly-builder/provision.sh` | makes the machine able to build; idempotent |

`ssh_port` in the cargo-remote config **overrides** the `Port` in `ssh_config`,
so the two must agree. It is 2222, not 22 — see "Hallpass" below.

## Usage

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo remote-3000 -r fly check  --workspace --all-targets
cargo remote-3000 -r fly clippy --workspace --all-targets -- --deny warnings
cargo remote-3000 -r fly test   --workspace
```

To get a binary back (needed by `make smoke`, which serves over HTTP):

```bash
cargo remote-3000 -r fly build -c=debug/surrealdb-ds-server -p surrealdb-ds-server
```

`-c` alone copies the whole target directory; `-c=debug/<name>` copies one
artifact. `deps/`, `build/`, `.fingerprint/` and `incremental/` are excluded
from the download, but a debug `surrealdb-ds-server` is ~1.8 GB, so copying the
whole tree back would defeat the purpose.

## Bringing the machine up

The machine's **root filesystem is ephemeral**. `apk`/`apt` packages do not
survive a stop; only the 50 GB volume at `/data` does. So after any restart:

```bash
flyctl machine start 841515b2730758 -a fly-builder-glowing-aurora-7598
flyctl machine exec -a fly-builder-glowing-aurora-7598 --no-container \
    841515b2730758 /data/bin/ensure-ready      # reinstalls packages, starts sshd
ssh fly-builder 'sh -s' < ops/fly-builder/provision.sh
```

`ensure-ready` is one executable rather than a script because `flyctl machine
exec` argv-splits instead of using a shell and so cannot express `sh -s`.

Stop it when idle — it bills while running:

```bash
flyctl machine stop 841515b2730758 -a fly-builder-glowing-aurora-7598
```

---

## What works, verified

- **Transfer and execution.** `cargo remote-3000 -r fly locate-project` returns
  `{"root":"/data/builds/3e982a840b1afbe5/Cargo.toml"}` — cargo genuinely ran on
  the machine.
- **Toolchain matches CI.** cargo/rustc 1.95.0, clippy 0.1.95, the version
  `ci.yml` pins. Pinned via the config's `toolchain` field, because
  `rustup default` with no argument resolves to stable and had installed 1.99.0.
- **Most of the dependency graph compiles remotely**, including the C++ ones
  (`librocksdb-sys`, wasmtime, vcpkg, bindgen).

## What does not work, and why

**The workspace cannot be built on Alpine/musl.** The build dies in
`rquickjs-sys`'s build script:

```
error: failed to run custom build command for `rquickjs-sys v0.11.0`
  panicked at bindgen-0.72.1/lib.rs:616:
  Unable to find libclang: "the `libclang` shared library at
  /usr/lib/libclang.so.17.0.5 could not be opened: Dynamic loading not supported"
```

That message reads like a missing package. It is not, and installing more
packages cannot fix it:

- bindgen is a build-dependency of both `surrealdb-librocksdb-sys` and
  `rquickjs-sys`, but only the former enables bindgen's `runtime` feature —
  via `surrealdb-rocksdb`'s `bindgen-runtime` feature. `cargo tree -e features
  -i bindgen` shows the chain.
- The workspace uses resolver `"3"`, which keeps those two feature sets
  separate, so `rquickjs-sys` gets a bindgen built **without** `runtime`.
- Without `runtime`, bindgen needs libclang *linkable at build time* rather than
  dlopen-able. That works on glibc — which is what this workstation and the
  upstream CI images are — and cannot work on musl.

`LIBCLANG_PATH=/usr/lib` is set in the machine's cargo config regardless: without
it Alpine's `libclang.so.17` naming defeats `clang-sys`'s search for the Debian
names, which is a real and separate problem on the RocksDB path.

### The fix, and why it is not applied

Build on glibc. Debian rather than Ubuntu, for parity with CI. This could not be
done non-destructively:

- `flyctl machine update --image debian:bookworm-slim` fails with *"deploying
  over the remote builder is not allowed"* — the app is registered as an org
  build machine, and Fly will not overwrite its own builder's image.
- Creating a second machine fails with *"remote builders may have only one
  volume"*, and the existing machine holds the only volume.
- So the Debian machine requires destroying the current one first, which also
  discards `/data` (rustup and the cargo registry have to be reinstalled).

That is an owner's decision, so it is not done here. See ADR-0009.

## Two traps worth keeping

**Hallpass is not a shell.** Fly's SSH server on port 22 (`Hallpass`) argv-splits
the command and execs it — `ssh host 'echo A; echo B'` prints `A; echo B`. It
does not read `authorized_keys` from disk either, so a key installed there is
ignored; auth is by certificate, issued with `flyctl ssh issue --agent`.

cargo-remote needs a login shell twice: rsync's `--rsync-path` is
`mkdir -p DIR && rsync`, and its build script is `cd DIR; . env; cargo ...`. So
the machine runs its own OpenSSH server on **2222**, configured from
`/data/ssh/sshd_config`, and cargo-remote points at that port. That server
authenticates the plain `~/.ssh/id_ed25519`, so nothing expires and there is no
dependency on the ssh-agent — unlike the certificate path.

**`rustup --no-modify-path` does not write `$CARGO_HOME/env`.** cargo-remote
sources `~/.cargo/env` by default, so without it every remote build fails with
`ash: cargo: not found`. `provision.sh` writes that file itself.

## Next step

Bake a Debian-based image with the toolchain, sshd, and `ensure-ready` already in
it, so the machine boots ready to build. That removes the ephemeral-root
reinstall (~60s), the single-token bootstrap, and the manual `ensure-ready` after
every restart. `ops/fly-builder/fly.glibc.toml` is the config for it.