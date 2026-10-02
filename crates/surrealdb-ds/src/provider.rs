//! `BackendProvider` — the plug-in seam into SurrealDB's path dispatch.
//!
//! This module is tied to the upstream contract. Under ADR-0002 Path B it is
//! deleted and replaced by whatever boundary type an independent implementation
//! needs.

use surrealdb_kvs::api::BoxFut;
use surrealdb_kvs::{Result, TransactionBuilder};
use surrealdb_kvs_any::{BackendProvider, ConnectContext};

use crate::SCHEMES;
use crate::builder::DsTransactionBuilder;

/// Provider registering the engine's path schemes.
///
/// Consulted in registration order; the first provider to claim a scheme wins,
/// so register this *after* any provider whose schemes you want to shadow.
#[derive(Debug, Clone, Default)]
pub struct DsBackend;

impl DsBackend {
	pub const fn new() -> Self {
		Self
	}
}

impl BackendProvider for DsBackend {
	fn schemes(&self) -> &[&'static str] {
		SCHEMES
	}

	/// Reject a bare `ds` with no `:` or `://` separator. Unlike `memory`, a
	/// bare scheme here is far more likely to be a user mistake than an
	/// intentional request for an ephemeral store.
	fn accepts_bare(&self) -> bool {
		false
	}

	/// Construct the engine.
	///
	/// Upstream's contract is explicit that this may perform "arbitrarily heavy
	/// asynchronous startup" and that the returned builder must be ready for use.
	/// Phase 0 does no real startup work; Phase 3 will join a cluster here and
	/// must not return until membership is settled and the node is routable.
	fn connect<'a>(
		&'a self,
		ctx: ConnectContext<'a>,
	) -> BoxFut<'a, Result<Box<dyn TransactionBuilder>>> {
		Box::pin(async move {
			tracing::info!(
				scheme = ctx.scheme,
				path = ctx.path,
				"surrealdb-ds: constructing engine"
			);
			let builder: Box<dyn TransactionBuilder> =
				Box::new(DsTransactionBuilder::connect(ctx).await?);
			Ok(builder)
		})
	}
}