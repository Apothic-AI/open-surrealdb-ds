//! Binary that registers the `surrealdb-ds` engine and serves SurrealQL on it.
//!
//! This exists instead of patching the upstream `surreal` binary because that
//! binary hard-codes its set of storage backends — there is no plugin discovery.
//! Building our own is ADR-0003.
//!
//! # What this is
//!
//! A SurrealDB server whose datastore can be our engine. `surrealdb_server::init`
//! boots the upstream CLI, so the command surface is upstream's:
//!
//! ```text
//! surrealdb-ds-server start [options] <path>
//! surrealdb-ds-server version
//! ```
//!
//! with `ds://` and `ds+mem://` understood in addition to every first-party
//! scheme. The seam is the composer (R-0036); see [`composer`].
//!
//! Two modes, because a server blocks and Phase 0's proof must not:
//!
//! - `construct-only <path>` — build the registry, construct through it, report,
//!   exit. This is what `make run` does: it proves registration works end to end
//!   without a socket, and it is fast enough for CI.
//! - anything else — handed to the upstream CLI verbatim, so `start` serves.
//!
//! Run `surrealdb-ds-server start --help` for the full option list.

use std::process::ExitCode;

use surrealdb_cnf::ConfigMap;
use surrealdb_ds_server::{DsComposer, registry};
use tokio_util::sync::CancellationToken;

/// Construct one backend through the public seam and report it. Exits non-zero on
/// failure, which is what makes it usable as a check rather than a demo.
async fn construct_only(path: &str) -> ExitCode {
	match registry().new_transaction_builder(path, CancellationToken::new(), ConfigMap::default()).await {
		Ok(builder) => {
			tracing::info!(name = builder.name(), "engine ready");
			println!("ok: constructed backend {} for {path}", builder.name());
			ExitCode::SUCCESS
		}
		Err(err) => {
			tracing::error!(%path, %err, "construction failed");
			eprintln!("error: {err}");
			ExitCode::FAILURE
		}
	}
}

// Deliberately NOT `#[tokio::main]`. `surrealdb_server::init` builds its own
// multi-threaded runtime and `block_on`s the CLI inside it, so calling it from
// within an existing runtime panics with "Cannot start a runtime from within a
// runtime". The construct-only path below therefore builds a short-lived runtime
// of its own rather than inheriting one.
fn main() -> ExitCode {
	let args: Vec<String> = std::env::args().collect();
	if args.get(1).map(String::as_str) == Some("construct-only") {
		let path = match args.get(2) {
			Some(path) => path.clone(),
			None => {
				eprintln!("usage: surrealdb-ds-server construct-only <path>");
				return ExitCode::FAILURE;
			}
		};
		// Logging is initialised here and ONLY here. The CLI installs its own
		// tracing subscriber from `--log`/`--log-format`, and two calls to
		// `set_global_default` panic — so a process that goes on to hand argv to
		// `init` must not have claimed it already.
		tracing_subscriber::fmt()
			.with_env_filter(
				tracing_subscriber::EnvFilter::try_from_default_env()
					.unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
			)
			.init();

		return match tokio::runtime::Builder::new_current_thread().enable_all().build() {
			Ok(runtime) => runtime.block_on(construct_only(&path)),
			Err(err) => {
				eprintln!("error: could not build a runtime: {err}");
				ExitCode::FAILURE
			}
		};
	}

	// Hand the real CLI our composer. It owns argv from here, blocks, and returns
	// the process exit code. Validation of our schemes happens inside it, through
	// the composer's `path_valid`.
	surrealdb_server::init(DsComposer::new())
}
