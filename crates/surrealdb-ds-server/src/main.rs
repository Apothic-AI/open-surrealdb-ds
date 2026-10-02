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
//! reports what it got. Serving HTTP is the next step, once we have confirmed
//! `surrealdb-server`'s public init path accepts a caller-constructed registry
//! (open question 3 in PROGRESS.md).

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
			// TODO(phase 0b): hand this builder to surrealdb-server's init path and
			// serve HTTP. Blocked on confirming the public init signature.
			ExitCode::SUCCESS
		}
		Err(err) => {
			tracing::error!(%path, %err, "construction failed");
			eprintln!("error: {err}");
			ExitCode::FAILURE
		}
	}
}