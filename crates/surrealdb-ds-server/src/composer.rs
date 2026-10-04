//! The composer: the one place our engine is handed to the rest of SurrealDB.
//!
//! `surrealdb_server::init` takes a single generic parameter implementing four
//! traits, and that parameter is the entire seam between our storage engine and
//! the real SurrealQL front end (R-0036). Upstream's own `CommunityComposer`
//! satisfies all four, so this type does the same thing and overrides exactly one
//! of them.
//!
//! ```text
//! init<C: TransactionBuilderFactory + RouterFactory + ConfigCheck
//!        + ObservabilityProvider>(composer: C)
//! ```
//!
//! | Trait | How it is satisfied |
//! | --- | --- |
//! | `TransactionBuilderFactory` | **Implemented here.** A `Backends` registry with the upstream community backends *and* ours, so `ds://` and `ds+mem://` construct through our provider while `memory` and `rocksdb:` keep working. |
//! | `RouterFactory` | Delegated — one call to `community_router`. |
//! | `ConfigCheck` | Delegated — community accepts every configuration. |
//! | `ObservabilityProvider` | Delegated — `create_observer` is the only required method and the rest are defaulted. |
//!
//! # Why delegate three and override one
//!
//! `community_router` is public and is what the `surreal` binary serves: `/health`,
//! `/ready`, `/version`, `/rpc`, `/export`, `/import`, `/status`, `/grpc`, `/sync`.
//! Reimplementing any of it would be reimplementing the front end, which ADR-0002
//! says not to do. One override, and the whole server comes with it.
//!
//! That is also the extension point for Phase 3: `community_router(&["/ready"])` is
//! the documented way for an edition to layer its own serve-readiness, which is
//! exactly where a node that must join a cluster before it can serve belongs.
//!
//! # Consequence worth knowing
//!
//! `init` boots the **upstream CLI**, so this binary is `surrealdb-ds-server
//! start [options] <path>` with `version`, `config` and the rest of the upstream
//! subcommands — not a bespoke command surface. See `scripts/smoke-http.sh`.

use axum::Router;
use std::sync::Arc;

use surrealdb_cnf::ConfigMap;
use surrealdb_ds::DsBackend;
use surrealdb_kvs_any::Backends;
use surrealdb_observe::ExecutionObserver;
use surrealdb_server::core::CommunityComposer;
use surrealdb_server::core::kvs::{TransactionBuilderFactory, TransactionBuilderParts};
use surrealdb_server::ntw::community_router;
use surrealdb_server::observe::ObservabilityProvider;
use surrealdb_server::{Config, RouterFactory};
use tokio_util::sync::CancellationToken;

/// A registry holding upstream's first-party backends plus ours.
///
/// Built per call rather than cached in the composer: `Backends` is a
/// `Vec<Box<dyn BackendProvider>>`, constructing one is a couple of allocations,
/// and holding it for the life of the process would put a `dyn` registry behind a
/// shared reference for no gain. Upstream's `CommunityComposer` does the same.
///
/// Our schemes (`ds`, `ds+mem`) do not collide with any first-party scheme, so
/// registering ours cannot shadow one — and registering after `community()`
/// means first-party schemes keep winning their own, which is what we want.
///
/// Public because the golden-file harness (`tests/golden.rs`) needs the same
/// registry the binary serves from, and because `construct-only` on the binary
/// needs it too: three copies of this list would be three places to change when
/// our schemes change.
pub fn registry() -> Backends<'static> {
	let mut backends = Backends::community();
	backends.register(DsBackend::new());
	backends
}

/// The composer handed to `surrealdb_server::init`.
pub struct DsComposer {
	/// Supplies the three traits we delegate. Nothing else; it carries no state.
	inner: CommunityComposer,
}

impl Default for DsComposer {
	fn default() -> Self {
		Self::new()
	}
}

impl DsComposer {
	pub const fn new() -> Self {
		Self { inner: CommunityComposer() }
	}
}

impl TransactionBuilderFactory for DsComposer {
	/// No router state to thread, matching `CommunityComposer`.
	type RouterState = ();

	/// Construct the storage backend for a datastore path.
	///
	/// This is the override that matters: it is the difference between a server
	/// that only knows `memory` and `rocksdb:` and one that also serves `ds://`.
	async fn new_transaction_builder(
		&self,
		path: &str,
		canceller: CancellationToken,
		config: ConfigMap,
	) -> anyhow::Result<TransactionBuilderParts<Self::RouterState>> {
		let builder = registry().new_transaction_builder(path, canceller, config).await?;
		// No router state: our builder produces no state the router factory needs
		// to be handed back.
		Ok(TransactionBuilderParts::without_router_state(builder))
	}

	/// Validate a datastore path, for the CLI's early check.
	fn path_valid(&self, v: &str) -> anyhow::Result<String> {
		Ok(registry().path_valid(v)?)
	}
}

impl RouterFactory for DsComposer {
	/// The community route set, unmodified.
	fn configure_router(_router_state: Self::RouterState) -> Router<Arc<surrealdb_server::RpcState>> {
		community_router(&[])
	}
}

#[async_trait::async_trait]
impl surrealdb_server::ConfigCheck for DsComposer {
	/// Community accepts every configuration, and so do we.
	///
	/// Phase 3 is where this earns content: a node that cannot serve until it has
	/// joined a cluster has configuration-dependent readiness, and `/ready` is
	/// where it should surface.
	async fn check_config(&mut self, cfg: &Config) -> anyhow::Result<()> {
		self.inner.check_config(cfg).await
	}
}

impl surrealdb_observe::ObservabilityProvider for DsComposer {
	fn create_observer(&self) -> Arc<dyn ExecutionObserver> {
		self.inner.create_observer()
	}
}

impl ObservabilityProvider for DsComposer {}

#[cfg(test)]
mod tests {
	use super::*;

	/// The router is the community one: `/health`, `/ready`, `/version` and
	/// `/rpc` must all be served, because those routes are the whole of Phase 0's
	/// last task and a composer that quietly dropped them would still compile.
	#[test]
	fn the_router_is_the_community_route_set() {
		// `Router` exposes no route enumeration, so assert the shape we can: it
		// builds without panicking, which is what a duplicate (path, method) in an
		// axum merge would cause. That the routes are the community ones is
		// `community_router`'s contract, and scripts/smoke-http.sh checks them for
		// real over HTTP.
		let _router: Router<Arc<surrealdb_server::RpcState>> = DsComposer::configure_router(());
	}

	/// Our schemes validate, and first-party ones still do.
	#[test]
	fn both_our_and_upstream_paths_are_accepted() {
		let composer = DsComposer::new();
		for path in ["ds+mem://", "ds://node1", "memory", "rocksdb:/tmp/x"] {
			assert!(composer.path_valid(path).is_ok(), "{path} must validate");
		}
		// And a shape that is neither must not.
		assert!(composer.path_valid("bogus://x").is_err());
	}
}
