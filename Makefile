# open-surrealdb-ds
#
# `make help` lists targets.

UPSTREAM_REF := v3.3.0
UPSTREAM_DIR := upstream/surrealdb

# Extra flags for every cargo invocation. Empty by default, so `make test` behaves
# exactly as typed. CI passes `--locked` to make a conformance result reproducible
# against the committed Cargo.lock; the upstream-drift workflow deliberately omits
# it, because there the whole point is to see new published versions.
CARGO_FLAGS ?=

# The cargo to invoke, as a single word. Override it to build on the shared Fly
# server instead of locally, which is required rather than merely faster on this
# workstation -- /home has been observed at 100%, so a local build cannot complete.
#
#   make test CARGO="cargo remote-3000 -r fly -d 1.95.0 --"
#
# The `--` matters: cargo-remote parses its own flags after the cargo subcommand
# too, and its `-p` is `--ssh-port`, so `... test -p some-crate` dies with
# `invalid value 'some-crate' for '--ssh-port <PORT>'`. `-d 1.95.0` is equally
# mandatory: the default is `stable`, and every build runs `rustup default`.
# See docs/remote-builds.md and the shared-flyio-build-server skill.
CARGO ?= cargo

# Machine identity for `make remote-stop`, and the default for `make remote`.
FLY_APP    ?= open-surrealdb-ds-builder
FLY_MACHINE ?= 87477e0cd01748
CARGO_REMOTE ?= cargo remote-3000 -r fly -d 1.95.0 --

.DEFAULT_GOAL := help
.PHONY: help bootstrap check test conformance conformance-list golden golden-update interop durability run construct smoke audit-upstream remote remote-stop clean

help: ## Show this help
	@grep -hE '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

bootstrap: ## Fetch + pin the upstream reference tree (tag v3.3.0)
	@./scripts/bootstrap.sh

check: ## Type-check every crate
	$(CARGO) $(CARGO_FLAGS) check --workspace --all-targets

test: ## Run the test suite (includes the upstream conformance suite)
	$(CARGO) $(CARGO_FLAGS) test --workspace

conformance: ## Run only the upstream KV backend conformance suite
	$(CARGO) $(CARGO_FLAGS) test -p surrealdb-ds --test kvs

conformance-list: ## List every conformance test and whether the suite will run it
	$(CARGO) $(CARGO_FLAGS) test -p surrealdb-ds --test kvs -- --list

golden: ## Round-trip a non-trivial dataset through upstream -> us -> upstream, byte for byte
	$(CARGO) $(CARGO_FLAGS) test -p surrealdb-ds-server --test golden -- --nocapture

golden-update: ## Rewrite the golden manifest from a fresh upstream run (review the diff)
	DS_GOLDEN_UPDATE=1 $(CARGO) $(CARGO_FLAGS) test -p surrealdb-ds-server --test golden -- --nocapture

interop: ## Open an upstream-written RocksDB directory with our own reader, byte for byte
	$(CARGO) $(CARGO_FLAGS) test -p surrealdb-ds-server --test interop -- --nocapture

durability: ## SIGKILL the store at each commit boundary and measure what survives (ADR-0012 step 2)
	$(CARGO) $(CARGO_FLAGS) test -p surrealdb-ds-server --test durability -- --nocapture

construct: ## Construct the engine from a path and exit, without serving
	$(CARGO) $(CARGO_FLAGS) run -q -p surrealdb-ds-server -- construct-only $(or $(PATH_ARG),ds+mem://)

run: ## Serve SurrealQL on `ds+mem://` over HTTP (blocks; Ctrl-C to stop)
	$(CARGO) $(CARGO_FLAGS) run -q -p surrealdb-ds-server -- start \
		--bind 127.0.0.1:8000 --username root --password root \
		$(or $(PATH_ARG),ds+mem://)

remote: ## Run one cargo command on the shared Fly server: make remote CMD='test --workspace'
	@$(CARGO_REMOTE) $(CMD)

remote-stop: ## Stop the build server so it stops billing
	@flyctl machine stop $(FLY_MACHINE) -a $(FLY_APP)

smoke: ## Start the server, exercise /health /ready /version and a /rpc round trip
	@./scripts/smoke-http.sh

audit-upstream: ## Re-check published SurrealDB crates and licences; rewrite docs/upstream-crates.md
	@./scripts/audit-upstream.sh

clean: ## Remove build artifacts
	cargo clean
