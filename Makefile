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
.PHONY: help bootstrap check test conformance conformance-list run audit-upstream clean

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

run: ## Construct our engine from `ds+mem://` and exit
	cargo $(CARGO_FLAGS) run -q -p surrealdb-ds-server -- ds+mem://

audit-upstream: ## Re-check published SurrealDB crates and licences; rewrite docs/upstream-crates.md
	@./scripts/audit-upstream.sh

clean: ## Remove build artifacts
	cargo clean
