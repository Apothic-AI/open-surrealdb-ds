//! `TransactionBuilder` — the datastore abstraction every backend implements.
//!
//! Tied to the upstream contract (ADR-0002 Path B would replace this).

use std::sync::Arc;

use surrealdb_kvs::api::BoxFut;
use surrealdb_kvs::builder::{Metric, Metrics};
use surrealdb_kvs::{Result, Transactable, TransactionBuilder, TransactionType};
use surrealdb_kvs_any::ConnectContext;

use crate::storage::{Counters, VersionedStore};
use crate::txn::DsTxn;

/// The metrics group this engine exports under.
///
/// The `surrealdb.ds.` prefix is the project's telemetry contract, adopted from
/// the distributed store's published metric scheme so the same dashboards and
/// alerts read our node.
const METRICS_GROUP: &str = "surrealdb.ds";

/// Constructs transactions against the engine's storage tier.
pub struct DsTransactionBuilder {
	store: Arc<VersionedStore>,
	/// Whether a transaction holds resources outside this process.
	///
	/// It does not: the tier is in this process's heap. Upstream's flag is about
	/// in-process optimisations a coordinator-side transaction still supports,
	/// not about physical locality — a distributed backend running single-node
	/// reports the same thing. Phase 3 replaces this with a real answer, which
	/// stays `true` for coordinator-side transactions.
	distributed: bool,
}

impl DsTransactionBuilder {
	/// Construct an engine over a fresh store.
	///
	/// `ctx.scheme` and `ctx.path` are accepted and not yet interpreted: the
	/// `ds+mem://` form forces the in-memory tier regardless of path, and Phase 1
	/// is where a path becomes a local-engine directory and `ctx.config` (which
	/// carries the connection string's `?key=value` parameters under
	/// `datastore_`-prefixed keys) becomes tuning and object-store endpoints.
	pub async fn connect(ctx: ConnectContext<'_>) -> Result<Self> {
		let _ = (ctx.scheme, ctx.path, &ctx.config);
		Ok(Self { store: Arc::new(VersionedStore::new()), distributed: false })
	}

	/// The metrics this engine publishes, and how to read each one.
	///
	/// Every name here is answered by [`DsTransactionBuilder::collect_u64_metric`]
	/// from a counter the storage tier actually maintains. A declared metric the
	/// tier cannot account for would be a metric reporting a constant, which is
	/// worse than not publishing it.
	const METRICS: &'static [Metric] = &[
		Metric {
			name: "surrealdb.ds.transactions_committed",
			description: "Transactions whose writes were made visible",
		},
		Metric {
			name: "surrealdb.ds.transactions_cancelled",
			description: "Transactions discarded before commit",
		},
		Metric {
			name: "surrealdb.ds.transactions_conflicted",
			description: "Commits refused because a key they had read had changed",
		},
		Metric {
			name: "surrealdb.ds.keys_written",
			description: "Keys created or overwritten by committed transactions",
		},
		Metric {
			name: "surrealdb.ds.keys_deleted",
			description: "Keys removed by committed transactions",
		},
		Metric {
			name: "surrealdb.ds.value_bytes_written",
			description: "Value bytes written by committed transactions",
		},
	];

	/// The metric set this engine publishes.
	fn metrics() -> Metrics {
		Metrics { name: METRICS_GROUP, u64_metrics: Self::METRICS.iter().map(|m| Metric { name: m.name, description: m.description }).collect() }
	}

	/// Read one counter by name.
	///
	/// An undeclared name is `None`, not `0`: the difference between "this
	/// backend does not publish it" and "it happens to be zero" is what lets a
	/// collector treat a missing series as absent rather than as a flat line.
	fn collect(counters: &Counters, metric: &str) -> Option<u64> {
		use std::sync::atomic::Ordering::Relaxed;
		Some(match metric {
			"surrealdb.ds.transactions_committed" => counters.commits.load(Relaxed),
			"surrealdb.ds.transactions_cancelled" => counters.cancels.load(Relaxed),
			"surrealdb.ds.transactions_conflicted" => counters.conflicts.load(Relaxed),
			"surrealdb.ds.keys_written" => counters.keys_written.load(Relaxed),
			"surrealdb.ds.keys_deleted" => counters.keys_deleted.load(Relaxed),
			"surrealdb.ds.value_bytes_written" => counters.value_bytes_written.load(Relaxed),
			_ => return None,
		})
	}
}

impl TransactionBuilder for DsTransactionBuilder {
	/// Create a backend transaction.
	///
	/// Returns the transaction and upstream's "is it local to the process" flag.
	fn new_transaction(
		&self,
		tx_type: TransactionType,
	) -> BoxFut<'_, Result<(Box<dyn Transactable>, bool)>> {
		Box::pin(async move {
			let txn: Box<dyn Transactable> = Box::new(DsTxn::begin(self.store.clone(), tx_type));
			Ok((txn, !self.distributed))
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

	/// The consensus, replication, recovery and GC instruments the distributed
	/// tier registers here.
	///
	/// Phase 0 publishes what the storage tier genuinely accounts for. The
	/// consensus-side counters — quorum size, prepare retries, view changes,
	/// recovery bytes replayed — have no honest value yet, so they are absent
	/// rather than zero.
	fn register_metrics(&self) -> Option<Metrics> {
		Some(Self::metrics())
	}

	fn collect_u64_metric(&self, metric: &str) -> Option<u64> {
		Self::collect(self.store.counters(), metric)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::Ordering;

	/// Every metric the builder declares must be readable, and nothing else may
	/// be. A declared-but-unreadable name is a series that silently stops.
	#[test]
	fn declared_metrics_are_collectable() {
		let metrics = DsTransactionBuilder::metrics();
		assert!(!metrics.name.is_empty());
		assert!(!metrics.u64_metrics.is_empty());
		for metric in &metrics.u64_metrics {
			assert!(
				DsTransactionBuilder::collect(&Counters::default(), metric.name).is_some(),
				"{}",
				metric.name
			);
		}
		assert!(DsTransactionBuilder::collect(&Counters::default(), "surrealdb.ds.nope").is_none());
	}

	/// The counters must move, or they are decoration.
	#[tokio::test]
	async fn counters_follow_what_the_tier_does() {
		let store = Arc::new(VersionedStore::new());
		let builder = DsTransactionBuilder { store: store.clone(), distributed: false };
		let counters = store.counters();

		let read = |name: &str| builder.collect_u64_metric(name).unwrap();

		let tx = DsTxn::begin(store.clone(), TransactionType::Write);
		assert_eq!(read("surrealdb.ds.transactions_committed"), 0);
		surrealdb_kvs::Transactable::set(&tx, b"k".as_slice().into(), b"v".to_vec())
			.await
			.unwrap();
		surrealdb_kvs::Transactable::commit(&tx).await.unwrap();
		assert_eq!(read("surrealdb.ds.transactions_committed"), 1);
		assert_eq!(read("surrealdb.ds.keys_written"), 1);
		assert_eq!(read("surrealdb.ds.value_bytes_written"), 1);

		let tx = DsTxn::begin(store.clone(), TransactionType::Write);
		surrealdb_kvs::Transactable::cancel(&tx).await.unwrap();
		assert_eq!(read("surrealdb.ds.transactions_cancelled"), 1);
		assert_eq!(counters.commits.load(Ordering::Relaxed), 1);
	}
}