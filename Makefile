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

.DEFAULT_GOAL := help
.PHONY: help bootstrap check test conformance conformance-list golden golden-update interop run construct smoke audit-upstream clean

help: ## Show this help
	@grep -hE '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

bootstrap: ## Fetch + pin the upstream reference tree (tag v3.3.0)
	@./scripts/bootstrap.sh

check: ## Type-check every crate
	cargo $(CARGO_FLAGS) check --workspace --all-targets

test: ## Run the test suite (includes the upstream conformance suite)
	cargo $(CARGO_FLAGS) test --workspace

conformance: ## Run only the upstream KV backend conformance suite
	cargo $(CARGO_FLAGS) test -p surrealdb-ds --test kvs

conformance-list: ## List every conformance test and whether the suite will run it
	cargo $(CARGO_FLAGS) test -p surrealdb-ds --test kvs -- --list

golden: ## Round-trip a non-trivial dataset through upstream -> us -> upstream, byte for byte
	cargo $(CARGO_FLAGS) test -p surrealdb-ds-server --test golden -- --nocapture

golden-update: ## Rewrite the golden manifest from a fresh upstream run (review the diff)
	DS_GOLDEN_UPDATE=1 cargo $(CARGO_FLAGS) test -p surrealdb-ds-server --test golden -- --nocapture

interop: ## Open an upstream-written RocksDB directory with our own reader, byte for byte
	cargo $(CARGO_FLAGS) test -p surrealdb-ds-server --test interop -- --nocapture

construct: ## Construct the engine from a path and exit, without serving
	cargo $(CARGO_FLAGS) run -q -p surrealdb-ds-server -- construct-only $(or $(PATH_ARG),ds+mem://)

run: ## Serve SurrealQL on `ds+mem://` over HTTP (blocks; Ctrl-C to stop)
	cargo $(CARGO_FLAGS) run -q -p surrealdb-ds-server -- start \
		--bind 127.0.0.1:8000 --username root --password root \
		$(or $(PATH_ARG),ds+mem://)

smoke: ## Start the server, exercise /health /ready /version and a /rpc round trip
	@./scripts/smoke-http.sh

audit-upstream: ## Re-check published SurrealDB crates and licences; rewrite docs/upstream-crates.md
	@./scripts/audit-upstream.sh

clean: ## Remove build artifacts
	cargo clean
