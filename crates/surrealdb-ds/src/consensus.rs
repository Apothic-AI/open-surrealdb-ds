//! Consensus — leaderless quorum commit (Phase 3).
//!
//! **No SurrealDB imports, by design.** This module and its siblings are the
//! part that survives an ADR-0002 move to a fully independent implementation.
//!
//! Design constraints derived from public sources and recorded in
//! `../../PROVENANCE.md`:
//!
//! - R-0006 commit on quorum acknowledgement, no elected leader
//! - R-0007 each availability zone runs its own write node; every write node can
//!   coordinate, so no leader bottleneck
//! - R-0004 odd node counts; an even count survives no more failures than N−1
//! - R-0005 the fast commit path needs every node to agree, so one slow node
//!   forces the slow path — which means the fast path has a bounded deadline and
//!   a defined fallback, not an indefinite wait
//! - R-0014 a transaction in flight across a membership change must rejoin the
//!   new membership or fail retryably, never stall
//! - R-0015 a membership-changing leader must not serve until a quorum of the
//!   new voter set holds the decided configuration
//! - R-0017 transaction write sets bounded by both operation count and bytes;
//!   over-bound fails fast at the coordinator, before any network traffic

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Consensus configuration for one group.
#[derive(Debug, Clone)]
pub struct Config {
	/// Node identifiers. Must be odd in size, and at least 3.
	pub members: Vec<NodeId>,
	/// Whether this node accepts writes and coordinates transactions.
	pub write_node: bool,
	/// Deadline for the fast path. On expiry, fall back to the slow path rather
	/// than waiting indefinitely — one slow peer must not stall the group.
	pub fast_quorum_timeout: Duration,
	/// Base and maximum backoff for prepare retries.
	pub retry_base: Duration,
	pub retry_max: Duration,
	/// Maximum operations in a single transaction write set.
	pub max_write_set_ops: u64,
	/// Maximum bytes in a single transaction write set.
	pub max_write_set_bytes: u64,
}

impl Config {
	/// Validate the configuration, returning a description of the first problem.
	pub fn validate(&self) -> Result<(), String> {
		if self.members.len() < 3 {
			return Err(format!("quorum group needs at least 3 members, got {}", self.members.len()));
		}
		if self.members.len().is_multiple_of(2) {
			return Err(format!(
				"quorum group should have an odd member count, got {}; an even count survives no more failures than N-1",
				self.members.len()
			));
		}
		if self.fast_quorum_timeout <= Duration::ZERO {
			return Err("fast quorum timeout must be positive".into());
		}
		if self.retry_base > self.retry_max {
			return Err("retry base exceeds retry max".into());
		}
		if self.max_write_set_ops == 0 || self.max_write_set_bytes == 0 {
			return Err("write set bounds must be non-zero (0 disables them; we do not support unbounded write sets)".into());
		}
		Ok(())
	}

	/// The number of acknowledgements required to commit.
	pub fn quorum(&self) -> usize {
		self.members.len() / 2 + 1
	}
}

pub type NodeId = u64;

/// Current view of cluster membership.
#[derive(Debug, Default)]
pub struct View {
	epoch: AtomicU64,
	members: RwLock<BTreeMap<NodeId, bool>>,
}

impl View {
	pub fn new(members: impl IntoIterator<Item = NodeId>) -> Arc<Self> {
		let this = Arc::new(Self::default());
		*this.members.write().expect("view lock poisoned") =
			members.into_iter().map(|id| (id, true)).collect();
		this
	}

	pub fn epoch(&self) -> u64 {
		self.epoch.load(Ordering::Acquire)
	}

	/// Live members in the current epoch.
	pub fn members(&self) -> Vec<NodeId> {
		self.members.read().expect("view lock poisoned").keys().copied().collect()
	}

	pub fn size(&self) -> usize {
		self.members.read().expect("view lock poisoned").len()
	}

	/// Install a new configuration under a fresh epoch.
	///
	/// R-0015: a node that has been superseded mid-transaction must abandon the
	/// round promptly rather than resend into a silent fence (R-0014), and a
	/// node that was fenced for being *ahead* of the cluster must escalate rather
	/// than starve. Both are Phase 3 work; this is the membership primitive.
	pub fn install(&self, epoch: u64, members: impl IntoIterator<Item = NodeId>) {
		self.epoch.store(epoch, Ordering::Release);
		let next: BTreeMap<NodeId, bool> = members.into_iter().map(|id| (id, true)).collect();
		*self.members.write().expect("view lock poisoned") = next;
	}
}

/// Tracks how far this node believes the group has advanced.
///
/// R-0016: recovery cost must be bounded by the log delta, never by dataset or
/// total history size. Every catch-up path here is delta-based by construction.
#[derive(Debug, Default)]
pub struct AppliedIndex {
	value: AtomicU64,
}

impl AppliedIndex {
	pub fn get(&self) -> u64 {
		self.value.load(Ordering::Acquire)
	}

	pub fn advance(&self, index: u64) {
		// Monotonic. A stale replica must never move the watermark backwards.
		self.value.fetch_max(index, Ordering::AcqRel);
	}

	/// How much catch-up remains. Bounded by design: this is a delta, not a
	/// dataset scan.
	pub fn lag(&self, group_high: u64) -> u64 {
		group_high.saturating_sub(self.get())
	}
}

/// Local stub of the consensus driver. Phase 3 replaces this body with the real
/// protocol; the types above are the part worth getting right first.
pub struct Engine {
	pub config: Config,
	pub view: Arc<View>,
	pub applied: AppliedIndex,
}

impl Engine {
	pub fn new(config: Config, view: Arc<View>) -> Self {
		Self {
			config,
			view,
			applied: AppliedIndex::default(),
		}
	}

	/// Whether this transaction's write set is within bounds.
	///
	/// R-0017: checked before any network traffic so an over-bound transaction
	/// fails fast at the coordinator rather than after a partial round.
	pub fn check_write_set(&self, ops: u64, bytes: u64) -> Result<(), WriteSetTooLarge> {
		if ops > self.config.max_write_set_ops {
			return Err(WriteSetTooLarge::TooManyOperations {
				got: ops,
				limit: self.config.max_write_set_ops,
			});
		}
		if bytes > self.config.max_write_set_bytes {
			return Err(WriteSetTooLarge::TooManyBytes {
				got: bytes,
				limit: self.config.max_write_set_bytes,
			});
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteSetTooLarge {
	TooManyOperations {
		got: u64,
		limit: u64,
	},
	TooManyBytes {
		got: u64,
		limit: u64,
	},
}