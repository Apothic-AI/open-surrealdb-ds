//! Binary that registers the `surrealdb-ds` engine and serves SurrealQL on it.
//!
//! This exists instead of patching the upstream `surreal` binary because that
//! binary hard-codes its set of storage backends — there is no plugin discovery.
//! Building our own is ADR-0003.
//!
//! # Status
//!
//! Phase 0. The engine is in-memory and single-node. This binary currently
//! proves only that **registration works end to end**: it builds a registry with
//! the upstream community backends plus ours, constructs through our scheme, and
//! reports what it got.
//!
//! Serving HTTP is the next step, and the seam is confirmed (R-0036):
//! `surrealdb_server::init` takes a composer, and `TransactionBuilderFactory` is
//! where a caller-built `Backends` registry goes — upstream's own
//! `CommunityComposer` is three lines of delegation to `Backends::community()`.
//! See DECISIONS.md ADR-0003.

use std::process::ExitCode;

use surrealdb_ds::DsBackend;
use surrealdb_cnf::config::ConfigMap;
use surrealdb_kvs_any::Backends;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> ExitCode {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
		)
		.init();

	// Start with upstream's first-party backends so `rocksdb://` keeps working,
	// then register ours. Registration order matters: first claimant of a scheme
	// wins, and our schemes (`ds`, `ds+mem`) do not collide with any first-party
	// scheme, so we cannot accidentally shadow or be shadowed.
	let mut backends = Backends::community();
	backends.register(DsBackend::new());

	let path = std::env::args().nth(1).unwrap_or_else(|| "ds+mem://".to_owned());

	match backends.new_transaction_builder(&path, CancellationToken::new(), ConfigMap::default()).await {
		Ok(builder) => {
			tracing::info!("engine ready: {}", builder.name());
			tracing::info!(%path, "constructed storage backend through our provider");
			println!("ok: constructed backend for {path}");
			// TODO(phase 0b): serve HTTP. Implement `TransactionBuilderFactory` over a
			// registry that has `DsBackend` registered, and hand a composer carrying it
			// to `surrealdb_server::init` — that is the whole seam (R-0036).
			ExitCode::SUCCESS
		}
		Err(err) => {
			tracing::error!(%path, %err, "construction failed");
			eprintln!("error: {err}");
			ExitCode::FAILURE
		}
	}
}