//! Replication — catch-up, anti-entropy, bounded recovery (Phase 3).
//!
//! **No SurrealDB imports, by design.** See ADR-0002.
//!
//! The governing constraint is R-0016, learned the hard way upstream: a
//! recovery drain that re-pages a peer's *entire outcome history* on every entry
//! into recovery is O(all transactions ever committed), which is why cold-start
//! convergence tails were measured in tens of seconds. Everything here is
//! delta-based by construction, and the drain size is a hard bound rather than
//! a consequence of the implementation.

use std::collections::VecDeque;

use crate::consensus::{AppliedIndex, NodeId};

/// A bounded per-node record of what we have written, so a recovering peer can
/// ask only for what we wrote since its last position.
#[derive(Debug)]
pub struct OutcomeJournal {
	capacity: usize,
	entries: VecDeque<(NodeId, u64)>,
}

impl OutcomeJournal {
	/// `capacity` bounds memory. Exhausting it degrades to full-stream fallback
	/// rather than to unbounded memory growth.
	pub fn new(capacity: usize) -> Self {
		Self {
			capacity,
			entries: VecDeque::with_capacity(capacity.min(4096)),
		}
	}

	/// Record that `to` has had our write at `index` applied.
	pub fn record(&mut self, to: NodeId, index: u64) {
		if self.entries.len() == self.capacity {
			self.entries.pop_front();
		}
		self.entries.push_back((to, index));
	}

	/// The highest index we know we have sent to `peer`, if we still remember.
	///
	/// `None` means the peer must be caught up from the beginning — which is
	/// exactly the case we are required never to pay for more than once.
	pub fn last_known_for(&self, peer: NodeId) -> Option<u64> {
		self.entries.iter().rev().find(|(n, _)| *n == peer).map(|(_, i)| *i)
	}

	pub fn len(&self) -> usize {
		self.entries.len()
	}

	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}
}

/// Catch-up plan for one peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUp {
	/// Peer is current; nothing to do.
	Current,
	/// Peer is behind; request the delta from its position to the group high.
	Delta { from: u64, to: u64 },
	/// We no longer remember the peer's position, or it is too far behind to be
	/// worth the delta. Fall back to a full stream.
	Full { from: u64 },
}

/// Decide how to bring `peer` up to date.
///
/// The delta branch is the normal case and must stay cheap. The full-stream
/// fallback is correct but must be rare — it is the branch whose cost upstream
/// accidentally made unbounded.
pub fn plan_catch_up(peer: &OutcomeJournal, local: &AppliedIndex, group_high: u64, delta_worthwhile_below: u64) -> CatchUp {
	let applied = local.get();
	if applied >= group_high {
		return CatchUp::Current;
	}
	match peer.last_known_for(local_id_placeholder()) {
		// Placeholder: real implementation keys on the peer id. Kept explicit so
		// the missing parameter is impossible to miss.
		None => CatchUp::Full { from: 0 },
		Some(from) => {
			if group_high.saturating_sub(from) > delta_worthwhile_below {
				CatchUp::Full { from }
			} else {
				CatchUp::Delta { from, to: group_high }
			}
		}
	}
}

fn local_id_placeholder() -> NodeId {
	0
}

/// How a peer's view of us compares to ours.
///
/// R-0013: replicas holding contradictory outcomes for one transaction must
/// converge and log both, rather than refusing to recover or install a view.
/// Divergence is a first-class, reportable state — not an error to be raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divergence {
	/// Peer matches us.
	Converged,
	/// Peer is behind by the given index delta.
	Behind { delta: u64 },
	/// Peer and we disagree on the outcome of a transaction.
	Contradictory { transaction: u64 },
}

impl Divergence {
	pub fn needs_reconciliation(&self) -> bool {
		matches!(self, Divergence::Contradictory { .. })
	}
}