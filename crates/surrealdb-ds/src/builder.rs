//! `TransactionBuilder` — the datastore abstraction every backend implements.
//!
//! Tied to the upstream contract (ADR-0002 Path B would replace this).

use std::sync::Arc;

use surrealdb_kvs::api::BoxFut;
use surrealdb_kvs::builder::Metrics;
use surrealdb_kvs::{Result, Transactable, TransactionBuilder, TransactionType};
use surrealdb_kvs_any::ConnectContext;

use crate::storage::MemoryStore;
use crate::txn::DsTxn;

/// Constructs transactions against the engine's storage tier.
pub struct DsTransactionBuilder {
	store: Arc<MemoryStore>,
	/// `false` for a transaction that must be coordinated across nodes.
	/// Phase 0 is always local.
	distributed: bool,
}

impl DsTransactionBuilder {
	pub async fn connect(ctx: ConnectContext<'_>) -> Result<Self> {
		// `ds+mem` forces the in-memory tier regardless of the path. Later
		// phases parse `ctx.config` for local-engine tuning and object-store
		// endpoints; today any path is in-memory.
		let _ = (ctx.scheme, ctx.path, &ctx.config);
		Ok(Self {
			store: Arc::new(MemoryStore::new()),
			distributed: false,
		})
	}
}

impl TransactionBuilder for DsTransactionBuilder {
	/// Create a backend transaction.
	///
	/// The returned flag is `true` when the transaction is local to the process
	/// and `false` when it holds external resources. Phase 0 is always local.
	/// Phase 3 must return `false` for anything touching the consensus group —
	/// upstream's contract ties this to liveness of long-running operations.
	fn new_transaction(
		&self,
		tx_type: TransactionType,
	) -> BoxFut<'_, Result<(Box<dyn Transactable>, bool)>> {
		Box::pin(async move {
			let txn: Box<dyn Transactable> = Box::new(DsTxn::begin(self.store.clone(), tx_type));
			Ok((txn, self.distributed))
		})
	}

	/// Engine name, surfaced in logs and diagnostics.
	fn name(&self) -> &'static str {
		"surrealds"
	}

	/// Release backend resources. Must be idempotent.
	fn shutdown(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move { Ok(()) })
	}

	/// Cluster metrics for the distributed tier.
	///
	/// Phase 3 registers consensus, view-change, recovery and GC instruments
	/// here, following the `surrealdb.ds.*` naming scheme.
	fn register_metrics(&self) -> Option<Metrics> {
		None
	}

	fn collect_u64_metric(&self, _metric: &str) -> Option<u64> {
		None
	}
}
