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
//! Phase 0 architecture spike. The storage engine is **in-memory only** and
//! single-node. Nothing here is correct, durable, or concurrent yet. See
//! `../../PLAN.md` and `../../PROGRESS.md`.
//!
//! # Module map
//!
//! Modules that must stay free of SurrealDB imports, because they are the part
//! that survives an ADR-0002 move to a fully independent implementation:
//!
//! - [`consensus`] — leaderless quorum commit, epochs, view change (Phase 3)
//! - [`replicate`] — catch-up, anti-entropy, bounded recovery drain (Phase 3)
//! - [`storage`] — local engine and object-storage durable tier (Phases 1, 4)
//!
//! Modules that are inherently tied to the upstream contract, and are the only
//! ones that would need rewriting under Path B:
//!
//! - [`provider`] — `BackendProvider` registration
//! - [`builder`] — `TransactionBuilder`
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
/// use surrealdb_kvs_any::Backends;
/// use surrealdb_ds::DsBackend;
///
/// let mut backends = Backends::community();
/// backends.register(DsBackend::new());
/// // "ds+mem://" now constructs through our engine.
/// let builder = backends.construct("ds+mem://").await.unwrap();
/// ```
pub use provider::DsBackend;