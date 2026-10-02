# open-surrealdb-ds
#
# `make help` lists targets.

UPSTREAM_REF := v3.3.0
UPSTREAM_DIR := upstream/surrealdb

.DEFAULT_GOAL := help
.PHONY: help bootstrap check test run audit-upstream clean

help: ## Show this help
	@grep -hE '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

bootstrap: ## Fetch and pin the upstream reference tree (tag v3.3.0)
	@./scripts/bootstrap.sh

check: ## Type-check every crate
	cargo check --workspace --all-targets

test: ## Run the test suite
	cargo test --workspace

run: ## Construct our engine from the `ds+mem://` scheme and exit
	cargo run -q -p surrealdb-ds-server -- ds+mem://

audit-upstream: ## Re-check published SurrealDB crates and their licences
	@./scripts/audit-upstream.sh

clean: ## Remove build artifacts
	cargo clean
