//! Runs the shared KV backend contract suite (`surrealdb-kvs-test`, vendored —
//! see ADR-0005) against our engine.
//!
//! One `TestBackend` is registered per engine under test, and the suite builds
//! it through the public `surrealdb-kvs-any` connection-string entry point, so
//! the path parser and the provider seam are covered on every run rather than
//! only when something is broken.
//!
//! # The backend name is part of the contract
//!
//! The suite matches `only` / `except` lists against the name a consumer
//! registers, and its vocabulary already contains `surrealds` — the
//! distributed store this project reimplements. Registering under that name is
//! what puts us behind the assertions the suite reserves for it, and it is the
//! honest name for the engine.
//!
//! What that costs us, stated plainly so nobody reads a green run as more than
//! it is:
//!
//! - `raw::getu_unsupported` **requires** `getu` to be refused with
//!   `UnsupportedLockedReads`, so the four `multi::getu_*` conflict tests and
//!   `raw::getu` / `raw::getu_readonly` are reported *ignored* rather than run.
//!   Locked reads stay an explicit gap until we implement them.
//! - `snapshot::write_skew_permitted` is *ignored* for `surrealds`, because the
//!   suite documents the distributed store as serializable. Nothing in the
//!   suite then holds us to that, so `crates/surrealdb-ds/tests/serializable.rs`
//!   asserts it ourselves.
//! - `multi::multiwriter_same_keys_allow` **requires** overlapping blind writes
//!   to the same key to all commit, last committer winning — while
//!   `multi::multiwriter_same_keys_conflict` is ignored for us. That
//!   combination (blind writes never conflict; reads are validated at commit)
//!   is the model, and it is not the same thing as first-committer-wins.

use std::process::ExitCode;

use surrealdb_cnf::ConfigMap;
use surrealdb_ds::DsBackend;
use surrealdb_kvs_any::Backends;
use tokio_util::sync::CancellationToken;

/// A registry holding upstream's first-party backends plus ours, so the run
/// proves coexistence rather than a registry containing only us.
fn backends() -> Backends<'static> {
	let mut backends = Backends::community();
	backends.register(DsBackend::new());
	backends
}

/// Construct our engine from a connection path, exactly as the server does.
async fn ds_from_path(path: &'static str) -> surrealdb_kvs_test::TestDs {
	let builder =
		backends().new_transaction_builder(path, CancellationToken::new(), ConfigMap::empty()).await.unwrap();
	surrealdb_kvs_test::TestDs::from_builder(builder)
}

fn main() -> ExitCode {
	let backends = vec![
		// The in-memory tier: what `ds+mem://` gives us. Every test builds a
		// fresh datastore through the factory, so tests never share state.
		surrealdb_kvs_test::TestBackend::new("surrealds", || ds_from_path("ds+mem://")),
	];
	surrealdb_kvs_test::run(backends)
}