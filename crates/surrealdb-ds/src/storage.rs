//! The storage tier: a versioned, snapshot-isolated keyspace.
//!
//! Deliberately holds **no SurrealDB imports** — this module is the Phase 1/4
//! seam where a real local engine and an object-storage durable tier get
//! substituted. See ADR-0002. Nothing here may mention `surrealdb_kvs`; the
//! boundary types it speaks are `Vec<u8>` keys and values, `u64` stamps, and the
//! [`ReadSet`] / [`Conflict`] pair.
//!
//! # Model
//!
//! Each key holds a list of committed versions in ascending stamp order, and a
//! tombstone is a version whose value is `None`. A transaction reads the latest
//! version at or below the stamp it captured when it began, so a reader that
//! starts before a writer commits keeps seeing the state it started with no
//! matter how long it lives. That is snapshot isolation, and it is what lets a
//! transaction be validated rather than trusted: because a snapshot is stable,
//! "was this key modified since my snapshot" is a question the commit path can
//! answer exactly.
//!
//! The clock is a single `u64` handed out under the same lock that appends
//! versions, so a commit's stamp orders it against every other commit without a
//! second round trip. Phase 3 replaces this with a quorum-assigned stamp, and
//! the reason is that a quorum stamp is **not** linear: the highest committed
//! stamp can float above one that has not been applied locally, which is exactly
//! why `safe_timestamp` cannot simply return `timestamp()` on a distributed
//! engine (R-0010).
//!
//! # Known limits
//!
//! - **In-memory only.** Nothing survives the process. Phase 1 adds a durable
//!   local engine; Phase 4 the object-storage tier.
//! - **No version GC.** Old versions accumulate for the life of the process, and
//!   so does the commit history that backs range validation. Phase 1, once there
//!   is a durable log to truncate against and a watermark to drop history below.
//! - **Range reads validate by walking the range.** Conflict detection for a
//!   recorded range costs one pass over it. That is proportionate — the caller
//!   asked for the range — but it is not what a distributed engine will do.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// One committed version of one key.
#[derive(Debug)]
struct Version {
	/// The commit stamp that produced this version. Ascending within a key.
	stamp: u64,
	/// The stored bytes, or `None` for a tombstone (the key was deleted here).
	val: Option<Vec<u8>>,
}

/// What a transaction read, for commit-time validation.
///
/// Every read a transaction performs is recorded, because under snapshot
/// isolation a read is only safe to act on if the data it saw is still the data
/// nobody else has replaced. Recording them is what makes that checkable rather
/// than assumed.
///
/// Point reads are tracked exactly. Ranges are tracked as bounds and checked by
/// walking them, which is exact too — the check is over the same span the caller
/// already asked to scan.
#[derive(Debug, Default)]
pub struct ReadSet {
	points: BTreeSet<Vec<u8>>,
	ranges: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ReadSet {
	pub fn new() -> Self {
		Self::default()
	}

	/// Record a read of one key.
	pub fn point(&mut self, key: &[u8]) {
		self.points.insert(key.to_vec());
	}

	/// Record a read of the half-open range `[start, end)`.
	pub fn range(&mut self, start: &[u8], end: &[u8]) {
		self.ranges.push((start.to_vec(), end.to_vec()));
	}

	pub fn is_empty(&self) -> bool {
		self.points.is_empty() && self.ranges.is_empty()
	}
}

/// A commit that cannot proceed because another transaction changed something
/// this one read.
#[derive(Debug)]
pub struct Conflict {
	/// The keys whose contents moved since the transaction's snapshot. Reported
	/// for diagnosis; the transaction must be retried from the beginning, not
	/// resumed.
	pub keys: Vec<Vec<u8>>,
}

/// Monotonic counters the engine exports under the `surrealdb.ds.*` scheme.
///
/// Counters live here, in the tier that increments them, and the builder only
/// maps names onto them. A declared metric that this struct cannot answer is a
/// metric that would report a lie.
#[derive(Debug, Default)]
pub struct Counters {
	pub commits: AtomicU64,
	pub cancels: AtomicU64,
	pub conflicts: AtomicU64,
	pub keys_written: AtomicU64,
	pub keys_deleted: AtomicU64,
	pub value_bytes_written: AtomicU64,
}

/// The versioned keyspace itself.
#[derive(Debug, Default)]
struct Inner {
	/// key -> its committed versions, ascending by stamp.
	versions: BTreeMap<Vec<u8>, Vec<Version>>,
	/// Highest stamp handed out so far. Also the read snapshot for a new
	/// transaction.
	clock: u64,
	/// Commit history, newest last, as `(stamp, key)`. Appended with the
	/// versions so range validation can ask "did anything land in this range
	/// after stamp X" without walking it.
	history: VecDeque<(u64, Vec<u8>)>,
}

impl Inner {
	/// The stamp of the newest version of `key`, or 0 when it has never been
	/// written.
	fn last_stamp(&self, key: &[u8]) -> u64 {
		self.versions.get(key).and_then(|v| v.last()).map_or(0, |v| v.stamp)
	}

	/// The value of `key` as of `at`: the newest version at or below it, or
	/// `None` for absent-or-deleted.
	fn read_at(&self, key: &[u8], at: u64) -> Option<Vec<u8>> {
		let versions = self.versions.get(key)?;
		versions
			.iter()
			.rev()
			.find(|v| v.stamp <= at)
			.and_then(|v| v.val.clone())
	}

	/// Every live pair in `[start, end)` as of `at`, in ascending key order.
	fn range_at(&self, start: &[u8], end: &[u8], at: u64) -> Vec<(Vec<u8>, Vec<u8>)> {
		self.versions
			.range(start.to_vec()..)
			.take_while(|(k, _)| k.as_slice() < end)
			.filter_map(|(k, versions)| {
				versions
					.iter()
					.rev()
					.find(|v| v.stamp <= at)
					.and_then(|v| v.val.clone())
					.map(|v| (k.clone(), v))
			})
			.collect()
	}
}

/// A shared, versioned keyspace.
#[derive(Debug, Default)]
pub struct VersionedStore {
	inner: Mutex<Inner>,
	counters: Counters,
}

/// The part of a transaction's write set the store applies: `None` is a delete.
pub type WriteSet = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

impl VersionedStore {
	pub fn new() -> Self {
		Self::default()
	}

	/// The stamp a transaction beginning now should read at.
	///
	/// Everything committed so far is visible; anything committed after this
	/// call is not, which is the whole of the snapshot guarantee.
	pub fn snapshot(&self) -> u64 {
		self.lock().clock
	}

	/// The value of `key` as of `at`.
	pub fn get(&self, key: &[u8], at: u64) -> Option<Vec<u8>> {
		self.lock().read_at(key, at)
	}

	/// Every live pair in `[start, end)` as of `at`, in ascending key order.
	pub fn range(&self, start: &[u8], end: &[u8], at: u64) -> Vec<(Vec<u8>, Vec<u8>)> {
		self.lock().range_at(start, end, at)
	}

	/// Apply `writes` atomically, or refuse.
	///
	/// Validation and application happen under one lock so a key cannot be
	/// modified between the check and the write — the gap a check-then-act
	/// implementation would leave open, and the shape of the lost-write race the
	/// upstream 3.3.0 notes record for their own mesh.
	///
	/// Returns the stamp the commit was assigned, or the keys that moved under
	/// the transaction. A refusal writes nothing.
	pub fn commit(
		&self,
		snapshot: u64,
		writes: &WriteSet,
		reads: &ReadSet,
	) -> Result<u64, Conflict> {
		let mut inner = self.lock();

		let mut keys = Vec::new();
		for key in reads.points.iter() {
			if inner.last_stamp(key) > snapshot {
				keys.push(key.clone());
			}
		}
		if keys.is_empty() {
			for (start, end) in reads.ranges.iter() {
				if let Some(key) = first_touched(&inner, start, end, snapshot) {
					keys.push(key);
				}
			}
		}
		if !keys.is_empty() {
			self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
			return Err(Conflict { keys });
		}

		// A commit that changed nothing gets no stamp: advancing the clock for
		// an empty write set would make every concurrent reader's next snapshot
		// later for nothing.
		if writes.is_empty() {
			return Ok(snapshot);
		}

		let stamp = inner.clock + 1;
		let mut written = 0u64;
		let mut deleted = 0u64;
		let mut bytes = 0u64;
		for (key, val) in writes {
			inner
				.versions
				.entry(key.clone())
				.or_default()
				.push(Version { stamp, val: val.clone() });
			inner.history.push_back((stamp, key.clone()));
			match val {
				Some(v) => {
					written += 1;
					bytes += v.len() as u64;
				}
				None => deleted += 1,
			}
		}
		inner.clock = stamp;
		drop(inner);

		self.counters.commits.fetch_add(1, Ordering::Relaxed);
		self.counters.keys_written.fetch_add(written, Ordering::Relaxed);
		self.counters.keys_deleted.fetch_add(deleted, Ordering::Relaxed);
		self.counters.value_bytes_written.fetch_add(bytes, Ordering::Relaxed);
		Ok(stamp)
	}

	/// Note that a transaction was discarded.
	pub fn note_cancel(&self) {
		self.counters.cancels.fetch_add(1, Ordering::Relaxed);
	}

	pub fn counters(&self) -> &Counters {
		&self.counters
	}

	fn lock(&self) -> MutexGuard<'_, Inner> {
		// A panic mid-commit leaves `inner` structurally intact (the map and the
		// clock are only ever updated together), so the poisoning this recovers
		// from is a poisoned warning, not lost data.
		self.inner.lock().unwrap_or_else(|e| e.into_inner())
	}
}

/// The first key in `[start, end)` with a version newer than `since`.
///
/// The history is walked in reverse and entries outside the range are skipped,
/// so the common case — the range was touched by nothing recent — costs the
/// number of commits since `since` rather than the size of the range.
fn first_touched(inner: &Inner, start: &[u8], end: &[u8], since: u64) -> Option<Vec<u8>> {
	for (stamp, key) in inner.history.iter().rev() {
		if *stamp <= since {
			return None;
		}
		if key.as_slice() >= start && key.as_slice() < end {
			return Some(key.clone());
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_snapshot_does_not_see_a_later_commit() {
		let store = VersionedStore::new();
		let at = store.snapshot();
		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), Some(b"v".to_vec()));
		store.commit(at, &writes, &ReadSet::new()).unwrap();
		assert_eq!(store.get(b"k", at), None, "a snapshot taken before the commit must not see it");
		assert_eq!(store.get(b"k", store.snapshot()), Some(b"v".to_vec()));
	}

	#[test]
	fn a_delete_is_a_tombstone_not_an_absence() {
		let store = VersionedStore::new();
		let at = store.snapshot();
		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), Some(b"v".to_vec()));
		store.commit(at, &writes, &ReadSet::new()).unwrap();

		let after_create = store.snapshot();
		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), None);
		store.commit(after_create, &writes, &ReadSet::new()).unwrap();

		assert_eq!(store.get(b"k", after_create), Some(b"v".to_vec()), "the older snapshot still reads the value");
		assert_eq!(store.get(b"k", store.snapshot()), None);
	}

	#[test]
	fn a_read_of_a_key_written_after_the_snapshot_conflicts() {
		let store = VersionedStore::new();
		let at = store.snapshot();

		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), Some(b"v".to_vec()));
		store.commit(store.snapshot(), &writes, &ReadSet::new()).unwrap();

		let mut reads = ReadSet::new();
		reads.point(b"k");
		assert!(store.commit(at, &WriteSet::new(), &reads).is_err(), "a stale read must not commit");
	}

	#[test]
	fn a_refused_commit_writes_nothing() {
		let store = VersionedStore::new();
		let at = store.snapshot();

		let mut other = WriteSet::new();
		other.insert(b"k".to_vec(), Some(b"first".to_vec()));
		store.commit(store.snapshot(), &other, &ReadSet::new()).unwrap();

		let mut reads = ReadSet::new();
		reads.point(b"k");
		let mut mine = WriteSet::new();
		mine.insert(b"other".to_vec(), Some(b"mine".to_vec()));
		assert!(store.commit(at, &mine, &reads).is_err());
		assert_eq!(store.get(b"other", store.snapshot()), None, "a refused commit must leave no trace");
	}

	#[test]
	fn a_range_read_conflicts_with_a_write_inside_it() {
		let store = VersionedStore::new();
		let at = store.snapshot();

		let mut writes = WriteSet::new();
		writes.insert(b"a/mid".to_vec(), Some(b"v".to_vec()));
		store.commit(store.snapshot(), &writes, &ReadSet::new()).unwrap();

		let mut reads = ReadSet::new();
		reads.range(b"a/", b"a0");
		assert!(store.commit(at, &WriteSet::new(), &reads).is_err());
	}

	#[test]
	fn a_range_read_ignores_writes_outside_it() {
		let store = VersionedStore::new();
		let at = store.snapshot();

		let mut writes = WriteSet::new();
		writes.insert(b"z/elsewhere".to_vec(), Some(b"v".to_vec()));
		store.commit(store.snapshot(), &writes, &ReadSet::new()).unwrap();

		let mut reads = ReadSet::new();
		reads.range(b"a/", b"a0");
		assert!(store.commit(at, &WriteSet::new(), &reads).is_ok(), "an unrelated write must not conflict");
	}

	#[test]
	fn a_blind_write_does_not_conflict_with_another_blind_write() {
		// The distributed store's documented model: overlapping writes to one key
		// all commit and serialise in stamp order, rather than one aborting.
		let store = VersionedStore::new();
		let first = store.snapshot();
		let second = store.snapshot();

		let mut a = WriteSet::new();
		a.insert(b"k".to_vec(), Some(b"a".to_vec()));
		let mut b = WriteSet::new();
		b.insert(b"k".to_vec(), Some(b"b".to_vec()));
		store.commit(first, &a, &ReadSet::new()).unwrap();
		store.commit(second, &b, &ReadSet::new()).unwrap();

		assert_eq!(store.get(b"k", store.snapshot()), Some(b"b".to_vec()), "the later stamp wins");
	}

	#[test]
	fn an_inverted_range_is_empty() {
		let store = VersionedStore::new();
		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), Some(b"v".to_vec()));
		store.commit(store.snapshot(), &writes, &ReadSet::new()).unwrap();
		assert!(store.range(b"z", b"a", store.snapshot()).is_empty());
	}
}