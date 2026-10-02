//! `surrealdb-ds` — a clean-room distributed storage engine for SurrealDB.
//!
//! # What this is
//!
//! SurrealDB ships a single-node storage tier in every build, and a distributed
//! one ("SurrealDS") only as a closed, licence-gated binary. Upstream v3.3.0
//! publishes a documented extension point for exactly this case:
//! [`surrealdb_kvs_any::BackendProvider`], a trait whose own documentation names
//! "the enterprise distributed store" as the intended external implementor.
//!
//! This crate implements that trait and the [`surrealdb_kvs::Transactable`]
//! contract behind it.
//!
//! # Current state
//!
//! Phase 0. The engine passes upstream's own backend contract suite — 77 of 77
//! runnable tests, against the published `surrealdb-kvs-test` vendored under
//! `../../vendor/`. Transactions are snapshot-isolated with staged writes,
//! read-your-writes, undo-log savepoints, and read-set validation at commit.
//!
//! The engine is still **in-memory and single-node**: one process, one `Mutex`,
//! no durability, no replication, no locked reads. `safe_timestamp` is the
//! single-node default and becomes a correctness bug the moment a second node
//! exists. See `../../PLAN.md` for what is next and `../../PROGRESS.md` for what
//! is verified — including what the conformance suite declines to check.
//!
//! # Module map
//!
//! Modules that must stay free of SurrealDB imports, because they are the part
//! that survives an ADR-0002 move to a fully independent implementation:
//!
//! - [`consensus`] — leaderless quorum commit, epochs, view change (Phase 3)
//! - [`replicate`] — catch-up, anti-entropy, bounded recovery drain (Phase 3)
//! - [`storage`] — the versioned keyspace and, later, the object-storage tier
//!   (Phases 1, 4)
//!
//! Modules that are inherently tied to the upstream contract, and are the only
//! ones that would need rewriting under Path B:
//!
//! - [`provider`] — `BackendProvider` registration
//! - [`builder`] — `TransactionBuilder`, and the `surrealdb.ds.*` metric surface
//! - [`txn`] — `Transactable`

pub mod builder;
pub mod consensus;
pub mod provider;
pub mod replicate;
pub mod storage;
pub mod txn;

/// The path schemes this engine claims.
///
/// `ds` — the real engine.
/// `ds+mem` — a memory-only configuration, useful for tests and for the
/// Phase 0 spike. Reserved for development; not a production storage tier.
pub const SCHEMES: &[&str] = &["ds", "ds+mem"];

/// Re-exported so callers do not need to depend on upstream directly.
pub use surrealdb_kvs_any::{BackendProvider, Backends, ConnectContext};
pub use surrealdb_kvs::{Transactable, TransactionBuilder};

/// The engine's provider. Register into a [`Backends`] registry to make the
/// [`SCHEMES`] schemes constructible.
///
/// ```no_run
/// use surrealdb_cnf::ConfigMap;
/// use surrealdb_ds::{Backends, DsBackend};
/// use tokio_util::sync::CancellationToken;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut backends = Backends::community();
/// backends.register(DsBackend::new());
///
/// // "ds+mem://" now constructs through our engine.
/// let builder = backends
///   .new_transaction_builder("ds+mem://", CancellationToken::new(), ConfigMap::empty())
///   .await?;
/// let (tx, local) = builder.new_transaction(surrealdb_kvs::TransactionType::Write).await?;
/// tx.set(b"k".as_slice().into(), b"v".to_vec()).await?;
/// tx.commit().await?;
/// # let _ = local;
/// # Ok(())
/// # }
/// ```
pub use provider::DsBackend;

/// The result type the engine's boundary types use.
pub type Result<T> = surrealdb_kvs::Result<T>;