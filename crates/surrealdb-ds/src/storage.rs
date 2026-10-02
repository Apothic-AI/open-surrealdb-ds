//! The storage tier: a versioned, snapshot-isolated keyspace.
//!
//! Deliberately holds **no SurrealDB imports** — this module is the Phase 1/4
//! seam where a real local engine and an object-storage durable tier get
//! substituted. See ADR-0002. Nothing here may mention `surrealdb_kvs`; the
//! boundary types it speaks are `Vec<u8>` keys and values, `u64` stamps, and the
//! [`ReadSet`] / [`Conflict`] / [`StorageError`] trio.
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
//! # What may be collected
//!
//! A live transaction holds a [`SnapshotPin`] on the stamp it reads at. Let
//! `horizon` be the oldest pinned stamp — or the clock, when nothing is pinned.
//! Collection keeps, per key, **the newest version at or below `horizon`, and
//! every version above it**, and drops the rest.
//!
//! *Why that is sufficient.* Every live transaction reads at some
//! `s >= horizon`, and a read at `s` answers with the newest version whose stamp
//! is at or below `s`. That version is either above the horizon, in which case
//! collection kept it, or it is not, in which case the newest version at or
//! below `s` **is** the newest at or below `horizon` — also kept. So every
//! answer a live transaction can ask for is the answer it would have been given
//! from the full history, and no live transaction can read a version that was
//! collected. That is the property worth the complexity: a stale read is a
//! recoverable inconvenience, a read of a version that no longer exists is
//! silent corruption.
//!
//! *What follows from it.* The newest version of a key is never collected — it
//! is either above the horizon or it *is* the newest at or below it — so
//! `last_stamp`, which is the whole of conflict validation, point and range
//! alike, is invariant under collection. And a key trimmed to a single
//! version keeps that version even when it is a tombstone: dropping the key
//! outright would be equivalent for reads, but it would erase the fact that the
//! key was ever written, and that fact is what `last_stamp` answers with.
//!
//! With nothing pinned the horizon is the clock, so every key collapses to its
//! newest version and no history survives: the correct amount, because nobody
//! can read any.
//!
//! # Known limits
//!
//! - **In-memory only.** Nothing survives the process. Phase 1 adds a durable
//!   local engine; Phase 4 the object-storage tier.
//! - **Collection is a walk over every live key, under the store lock.** It runs
//!   after a commit that wrote something and whenever a pin is released, and is
//!   skipped while the horizon has not moved — everything committed in between is
//!   above the horizon, so there is nothing new to drop. Each *new* horizon still
//!   costs one pass over the keyspace, so a long-lived reader that keeps the
//!   horizon back makes every advance pay it. A real engine collects from its
//!   local store's own compaction, off the request path.
//! - **A transaction that is never dropped holds the horizon for ever.** The pin
//!   lives as long as the [`SnapshotPin`], and nothing reclaims it: a transaction
//!   leaked with `mem::forget`, a `Box<dyn Transactable>` retained past its
//!   commit, or a query that never completes all stop collection at their stamp,
//!   and versions committed above it accumulate without bound — which is the
//!   behaviour collection replaced. A transaction timeout, plus an engine that
//!   lets the query layer drop its handle, is the fix; neither exists here.
//! - **Range reads validate by walking the range.** Conflict detection for a
//!   recorded range checks each key's newest version, so it costs one pass over
//!   the range rather than the number of commits since the snapshot — the commit
//!   history that used to buy the cheaper trade is gone. That is proportionate,
//!   the caller asked for the range, but it is not what a distributed engine
//!   will do.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

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

/// Why a store operation did not complete.
#[derive(Debug)]
pub enum StorageError {
	/// Something this transaction read has changed since its snapshot. Retry the
	/// transaction from the beginning.
	Conflict(Conflict),
	/// The store's lock was poisoned by a panic that unwound through it.
	///
	/// What the store holds behind a poisoned lock is not something this module
	/// can vouch for, so it refuses everything from then on. The alternative —
	/// recovering the guard and carrying on — is the claim that a panic mid-commit
	/// cannot leave the keyspace and the clock disagreeing, and that claim is
	/// false: `Inner::clock` is advanced after the write loop, so a panic inside
	/// it leaves keys carrying versions at a stamp the clock never reached, and
	/// the next commit hands out that same stamp and writes a second version on
	/// top of the first. Refusing is the honest answer; the two states are
	/// indistinguishable from here.
	Poisoned,
}

impl std::fmt::Display for StorageError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			StorageError::Conflict(_) => f.write_str("a key this transaction read changed since its snapshot"),
			StorageError::Poisoned => f.write_str("the storage tier was poisoned by a panic and serves nothing"),
		}
	}
}

impl std::error::Error for StorageError {}

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
	/// How many live transactions are pinned at each snapshot stamp, ascending.
	/// The first key is the horizon collection drops below.
	///
	/// Deliberately a count per stamp rather than a running minimum. A running
	/// minimum has to be lowered on acquire and raised on release, and the raise
	/// is where a stale horizon hides: forget one and collection silently stops at
	/// a stamp nobody reads at, or worse, overshoots one somebody does. Here the
	/// minimum is simply the first key, an entry exists only while somebody holds
	/// that stamp, and the only way it can be wrong is for the map itself to be
	/// wrong — which `acquire` and `release` are the only two things that touch.
	live: BTreeMap<u64, usize>,
	/// The horizon the last automatic collection ran at.
	///
	/// Collection at a horizon is idempotent — everything committed since is
	/// above the horizon, so a horizon that has not moved has nothing new to drop
	/// — and remembering it is what stops a long-lived reader turning every
	/// commit into a walk over the whole keyspace.
	collected_at: Option<u64>,
}

impl Inner {
	/// The oldest stamp any live transaction can still read at, or the clock when
	/// nothing is pinned.
	fn horizon(&self) -> u64 {
		self.live.keys().next().copied().unwrap_or(self.clock)
	}

	/// The stamp of the newest version of `key`, or 0 when it has never been
	/// written.
	///
	/// Collection-invariant: collection never drops a key's newest version. See
	/// the module docs.
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

	/// Drop every version no live snapshot can reach, and report how many went.
	///
	/// Keeps, per key, the newest version at or below `horizon` and every version
	/// above it. A key whose versions are all above the horizon keeps all of them,
	/// and a key trimmed to one version keeps that version even if it is a
	/// tombstone.
	fn collect(&mut self, horizon: u64) -> usize {
		let mut dropped = 0;
		for versions in self.versions.values_mut() {
			if let Some(keep) = versions.iter().rposition(|version| version.stamp <= horizon) {
				dropped += keep;
				versions.drain(..keep);
			}
		}
		dropped
	}

	/// [`Inner::collect`] at the current horizon, skipped when the last run already
	/// used it. Reports how many versions went.
	fn collect_if_needed(&mut self) -> usize {
		let horizon = self.horizon();
		if self.collected_at == Some(horizon) {
			return 0;
		}
		self.collected_at = Some(horizon);
		self.collect(horizon)
	}
}

/// A shared, versioned keyspace.
#[derive(Debug, Default)]
pub struct VersionedStore {
	inner: Mutex<Inner>,
	/// Set once a panic has unwound through the lock, and never cleared.
	///
	/// Sticky on purpose: the keyspace behind a poisoned lock is not something
	/// this module can reason about, so the store stays unusable for the life of
	/// the process rather than resume on the next commit.
	poisoned: AtomicBool,
	counters: Counters,
}

/// A snapshot held open, keeping every version it can reach alive in the store.
///
/// Dropping it releases the hold, which is the only thing that lets collection
/// move past the stamp. It borrows the store rather than owning it, so a pin
/// that outlives its store cannot keep that store alive — there would be nothing
/// left to keep it alive for.
pub struct SnapshotPin {
	stamp: u64,
	store: Weak<VersionedStore>,
}

impl SnapshotPin {
	/// The stamp this snapshot reads at.
	pub(crate) fn stamp(&self) -> u64 {
		self.stamp
	}
}

impl Drop for SnapshotPin {
	fn drop(&mut self) {
		// A dropped store has nothing left to collect, so an expired borrow is a
		// no-op rather than a failure.
		if let Some(store) = self.store.upgrade() {
			store.release(self.stamp);
		}
	}
}

/// The part of a transaction's write set the store applies: `None` is a delete.
pub type WriteSet = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

/// One live key and the value it holds, in ascending key order. A tombstone is
/// not a pair and does not appear.
pub type Pair = (Vec<u8>, Vec<u8>);

impl VersionedStore {
	pub fn new() -> Self {
		Self::default()
	}

	/// Pin the snapshot a transaction beginning now should read at, until the
	/// returned pin is dropped.
	///
	/// This is the only way to hold a stamp across a commit, and the only way to
	/// obtain one at all: a stamp that is not pinned is correct for a read taken
	/// before the next commit and wrong for anything longer, so handing one out
	/// would be handing out a way to read a version collection has taken away.
	pub fn acquire(self: &Arc<Self>) -> Result<SnapshotPin, StorageError> {
		let mut inner = self.lock()?;
		let stamp = inner.clock;
		*inner.live.entry(stamp).or_default() += 1;
		Ok(SnapshotPin { stamp, store: Arc::downgrade(self) })
	}

	/// The stamp a transaction beginning now should read at, **without** pinning
	/// it.
	///
	/// Test-only, deliberately. An unpinned stamp is correct for a read taken
	/// before the next commit and wrong for anything longer, and a way to obtain
	/// one is a way to read at a stamp collection may move past while it is still
	/// being read at. Everything that outlives a single call takes an
	/// [`Self::acquire`] pin instead.
	#[cfg(test)]
	fn snapshot(&self) -> Result<u64, StorageError> {
		Ok(self.lock()?.clock)
	}

	/// The value of `key` as of `at`.
	pub fn get(&self, key: &[u8], at: u64) -> Result<Option<Vec<u8>>, StorageError> {
		Ok(self.lock()?.read_at(key, at))
	}

	/// Every live pair in `[start, end)` as of `at`, in ascending key order.
	pub fn range(
		&self,
		start: &[u8],
		end: &[u8],
		at: u64,
	) -> Result<Vec<Pair>, StorageError> {
		Ok(self.lock()?.range_at(start, end, at))
	}

	/// Apply `writes` atomically, or refuse.
	///
	/// Validation and application happen under one lock so a key cannot be
	/// modified between the check and the write — the gap a check-then-act
	/// implementation would leave open, and the shape of the lost-write race the
	/// upstream 3.3.0 notes record for their own mesh.
	///
	/// Returns the stamp the commit was assigned, or the keys that moved under
	/// the transaction. A refusal writes nothing. Collection runs in the same
	/// critical section as the write, so no reader can see the keyspace between
	/// the new versions landing and the unreachable ones going.
	pub fn commit(
		&self,
		snapshot: u64,
		writes: &WriteSet,
		reads: &ReadSet,
	) -> Result<u64, StorageError> {
		let mut inner = self.lock()?;

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
			return Err(StorageError::Conflict(Conflict { keys }));
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
			match val {
				Some(v) => {
					written += 1;
					bytes += v.len() as u64;
				}
				None => deleted += 1,
			}
		}
		inner.clock = stamp;
		inner.collect_if_needed();
		drop(inner);

		self.counters.commits.fetch_add(1, Ordering::Relaxed);
		self.counters.keys_written.fetch_add(written, Ordering::Relaxed);
		self.counters.keys_deleted.fetch_add(deleted, Ordering::Relaxed);
		self.counters.value_bytes_written.fetch_add(bytes, Ordering::Relaxed);
		Ok(stamp)
	}

	/// Note that a transaction was discarded.
	///
	/// Stays infallible even on a poisoned store: it counts discards that
	/// happened, and a counter that refused to answer would report less than it
	/// already knows.
	pub fn note_cancel(&self) {
		self.counters.cancels.fetch_add(1, Ordering::Relaxed);
	}

	pub fn counters(&self) -> &Counters {
		&self.counters
	}

	/// Collect now, and report how many versions it dropped.
	///
	/// Committing and releasing a pin already collect; this is the same work run
	/// on demand. It keeps to the same horizon, so it cannot invalidate a live
	/// transaction either.
	pub fn collect(&self) -> Result<usize, StorageError> {
		let mut inner = self.lock()?;
		let horizon = inner.horizon();
		Ok(inner.collect(horizon))
	}

	fn lock(&self) -> Result<MutexGuard<'_, Inner>, StorageError> {
		if self.poisoned.load(Ordering::Acquire) {
			return Err(StorageError::Poisoned);
		}
		match self.inner.lock() {
			Ok(inner) => Ok(inner),
			Err(_) => {
				// The first operation to meet the poison marks the store, and
				// every one after it fails without touching the lock again.
				self.poisoned.store(true, Ordering::Release);
				Err(StorageError::Poisoned)
			}
		}
	}

	/// Give up a pin, and collect if it was the one holding the horizon back.
	fn release(&self, stamp: u64) {
		// This runs from `Drop`, possibly while a panic is unwinding. It has
		// nowhere to report a failure, so it takes what it can and stops.
		let Ok(mut inner) = self.lock() else { return };
		match inner.live.get_mut(&stamp) {
			Some(count) => *count = count.saturating_sub(1),
			// Not a stamp this store handed out. Leaving it absent matters:
			// re-inserting it would pin the horizon at a stamp nobody reads at
			// and stop collection for the life of the process.
			None => return,
		}
		if inner.live.get(&stamp).is_some_and(|count| *count == 0) {
			inner.live.remove(&stamp);
		}
		inner.collect_if_needed();
	}
}

/// The first key in `[start, end)` with a version newer than `since`.
///
/// Versions ascend, so a key's newest version is its last, and it is the last
/// that answers this: an older version being newer than `since` implies the
/// newest one is too. Collection never drops the newest version, so this is
/// unaffected by it — which is the reason the commit history this replaced could
/// go without a replacement that walks the same range.
fn first_touched(inner: &Inner, start: &[u8], end: &[u8], since: u64) -> Option<Vec<u8>> {
	for (key, versions) in inner.versions.range(start.to_vec()..).take_while(|(key, _)| key.as_slice() < end) {
		if versions.last().is_some_and(|version| version.stamp > since) {
			return Some(key.clone());
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A store in the shape the transaction layer uses it: shared, so a
	/// [`SnapshotPin`] can be taken against it.
	fn store() -> Arc<VersionedStore> {
		Arc::new(VersionedStore::new())
	}

	/// Every version the store holds, over every key.
	fn versions(store: &VersionedStore) -> usize {
		store.lock().unwrap().versions.values().map(Vec::len).sum()
	}

	/// How many snapshots are currently pinned.
	fn pinned(store: &VersionedStore) -> usize {
		store.lock().unwrap().live.values().sum()
	}

	/// Commit a one-key write set at the current clock.
	fn write(store: &VersionedStore, key: &[u8], val: &[u8]) {
		let mut writes = WriteSet::new();
		writes.insert(key.to_vec(), Some(val.to_vec()));
		store.commit(store.snapshot().unwrap(), &writes, &ReadSet::new()).unwrap();
	}

	#[test]
	fn a_snapshot_does_not_see_a_later_commit() {
		let store = store();
		let reader = store.acquire().unwrap();
		write(&store, b"k", b"v");
		assert_eq!(store.get(b"k", reader.stamp()).unwrap(), None, "a snapshot taken before the commit must not see it");
		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), Some(b"v".to_vec()));
	}

	#[test]
	fn a_delete_is_a_tombstone_not_an_absence() {
		let store = store();
		write(&store, b"k", b"v");

		let after_create = store.acquire().unwrap();
		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), None);
		store.commit(store.snapshot().unwrap(), &writes, &ReadSet::new()).unwrap();

		assert_eq!(store.get(b"k", after_create.stamp()).unwrap(), Some(b"v".to_vec()), "the older snapshot still reads the value");
		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), None);
	}

	#[test]
	fn a_read_of_a_key_written_after_the_snapshot_conflicts() {
		let store = store();
		let reader = store.acquire().unwrap();

		write(&store, b"k", b"v");

		let mut reads = ReadSet::new();
		reads.point(b"k");
		assert!(store.commit(reader.stamp(), &WriteSet::new(), &reads).is_err(), "a stale read must not commit");
	}

	#[test]
	fn a_refused_commit_writes_nothing() {
		let store = store();
		let reader = store.acquire().unwrap();

		write(&store, b"k", b"first");

		let mut reads = ReadSet::new();
		reads.point(b"k");
		let mut mine = WriteSet::new();
		mine.insert(b"other".to_vec(), Some(b"mine".to_vec()));
		assert!(store.commit(reader.stamp(), &mine, &reads).is_err());
		assert_eq!(store.get(b"other", store.snapshot().unwrap()).unwrap(), None, "a refused commit must leave no trace");
	}

	#[test]
	fn a_range_read_conflicts_with_a_write_inside_it() {
		let store = store();
		let reader = store.acquire().unwrap();

		write(&store, b"a/mid", b"v");

		let mut reads = ReadSet::new();
		reads.range(b"a/", b"a0");
		assert!(store.commit(reader.stamp(), &WriteSet::new(), &reads).is_err());
	}

	#[test]
	fn a_range_read_ignores_writes_outside_it() {
		let store = store();
		let reader = store.acquire().unwrap();

		write(&store, b"z/elsewhere", b"v");

		let mut reads = ReadSet::new();
		reads.range(b"a/", b"a0");
		assert!(store.commit(reader.stamp(), &WriteSet::new(), &reads).is_ok(), "an unrelated write must not conflict");
	}

	#[test]
	fn a_blind_write_does_not_conflict_with_another_blind_write() {
		// The distributed store's documented model: overlapping writes to one key
		// all commit and serialise in stamp order, rather than one aborting.
		let store = store();
		let first = store.acquire().unwrap();
		let second = store.acquire().unwrap();

		let mut a = WriteSet::new();
		a.insert(b"k".to_vec(), Some(b"a".to_vec()));
		let mut b = WriteSet::new();
		b.insert(b"k".to_vec(), Some(b"b".to_vec()));
		store.commit(first.stamp(), &a, &ReadSet::new()).unwrap();
		store.commit(second.stamp(), &b, &ReadSet::new()).unwrap();

		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), Some(b"b".to_vec()), "the later stamp wins");
	}

	#[test]
	fn an_inverted_range_is_empty() {
		let store = store();
		write(&store, b"k", b"v");
		assert!(store.range(b"z", b"a", store.snapshot().unwrap()).unwrap().is_empty());
	}

	/// The case collection exists to protect: a transaction holding an old
	/// snapshot still reads what it began with, after collection has run.
	#[test]
	fn a_pinned_snapshot_still_reads_after_collection() {
		let store = store();
		write(&store, b"k", b"v1");
		write(&store, b"k", b"v2");

		let reader = store.acquire().unwrap();
		let at = reader.stamp();
		write(&store, b"k", b"v3");
		write(&store, b"k", b"v4");

		assert_eq!(store.collect().unwrap(), 0, "nothing at or below the horizon is collectable while it is pinned");
		assert_eq!(versions(&store), 3, "v1 is gone; v2 survives as the newest at or below the horizon");
		assert_eq!(store.get(b"k", at).unwrap(), Some(b"v2".to_vec()), "the pinned snapshot reads the value it began with");
		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), Some(b"v4".to_vec()));

		// Nothing pins the horizon any more, so the rest goes too.
		drop(reader);
		assert_eq!(versions(&store), 1);
		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), Some(b"v4".to_vec()));
	}

	#[test]
	fn collection_without_live_transactions_keeps_one_version_per_key() {
		let store = store();
		write(&store, b"a", b"1");
		write(&store, b"a", b"2");
		write(&store, b"a", b"3");
		write(&store, b"b", b"1");
		write(&store, b"b", b"2");

		assert_eq!(pinned(&store), 0);
		assert_eq!(store.collect().unwrap(), 0, "the commit path already collected");
		assert_eq!(versions(&store), 2, "five commits over two keys leave the newest of each");
		assert_eq!(store.get(b"a", store.snapshot().unwrap()).unwrap(), Some(b"3".to_vec()));
		assert_eq!(store.get(b"b", store.snapshot().unwrap()).unwrap(), Some(b"2".to_vec()));
	}

	/// A tombstone is the last version of its key and stays: dropping it would
	/// lose the fact the key was written, which is what `last_stamp` reports.
	#[test]
	fn collection_keeps_a_lone_tombstone() {
		let store = store();
		write(&store, b"k", b"v");
		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), None);
		let tombstoned = store.commit(store.snapshot().unwrap(), &writes, &ReadSet::new()).unwrap();

		assert_eq!(versions(&store), 1, "the value was collected, the tombstone was not");
		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), None);
		assert_eq!(
			store.lock().unwrap().last_stamp(b"k"),
			tombstoned,
			"the tombstone still carries the stamp conflict validation reads"
		);
	}

	/// A transaction that is simply dropped — no commit, no cancel — must release
	/// its pin, or collection stops at its stamp for good.
	#[test]
	fn a_dropped_pin_releases_without_commit_or_cancel() {
		let store = store();
		write(&store, b"k", b"v1");

		let reader = store.acquire().unwrap();
		assert_eq!(pinned(&store), 1);
		write(&store, b"k", b"v2");
		assert_eq!(versions(&store), 2, "the pin keeps the version it can still reach");

		drop(reader);
		assert_eq!(pinned(&store), 0);
		assert_eq!(versions(&store), 1);
	}

	/// Two transactions at the same stamp share one entry, and the last one out
	/// is what frees it.
	#[test]
	fn two_pins_at_one_stamp_release_independently() {
		let store = store();
		write(&store, b"k", b"v1");
		let older = store.acquire().unwrap();
		let newer = store.acquire().unwrap();
		assert_eq!(pinned(&store), 2);

		drop(newer);
		assert_eq!(pinned(&store), 1, "one pin is still holding the stamp");
		drop(older);
		assert_eq!(pinned(&store), 0);
	}

	#[test]
	fn a_refused_commit_leaves_no_version_behind() {
		let store = store();
		let reader = store.acquire().unwrap();
		write(&store, b"k", b"first");
		let before = versions(&store);

		let mut reads = ReadSet::new();
		reads.point(b"k");
		let mut mine = WriteSet::new();
		mine.insert(b"other".to_vec(), Some(b"mine".to_vec()));
		mine.insert(b"k".to_vec(), Some(b"clobber".to_vec()));
		assert!(matches!(store.commit(reader.stamp(), &mine, &reads), Err(StorageError::Conflict(_))));

		assert_eq!(versions(&store), before, "neither the new key nor the clobbering one was written");
		assert_eq!(store.get(b"other", store.snapshot().unwrap()).unwrap(), None);
		assert_eq!(store.get(b"k", store.snapshot().unwrap()).unwrap(), Some(b"first".to_vec()));
		assert_eq!(store.collect().unwrap(), 0, "a refusal changes nothing collection can act on");

		// And it leaves nothing behind for collection to find later either.
		drop(reader);
		store.collect().unwrap();
		assert_eq!(versions(&store), 1, "the refused transaction's key never existed");
		assert_eq!(store.get(b"other", store.snapshot().unwrap()).unwrap(), None);
	}

	/// Range validation reads the newest version of each key in the range, which
	/// is exactly what collection leaves behind — so it must keep working after it.
	#[test]
	fn range_validation_still_works_after_collection() {
		let store = store();
		write(&store, b"a/inside", b"1");
		write(&store, b"a/inside", b"2");
		write(&store, b"a/inside", b"3");
		write(&store, b"z/outside", b"1");
		assert_eq!(versions(&store), 2, "four commits collapsed to one version per key before anything was pinned");

		let reader = store.acquire().unwrap();
		let at = reader.stamp();
		write(&store, b"a/inside", b"4");
		store.collect().unwrap();
		assert_eq!(versions(&store), 3, "the pinned version and the one above it are all that is left of a/inside");

		let mut reads = ReadSet::new();
		reads.range(b"a/", b"a0");
		assert!(
			matches!(store.commit(at, &WriteSet::new(), &reads), Err(StorageError::Conflict(_))),
			"a write inside the range after the snapshot must still conflict"
		);

		let mut reads = ReadSet::new();
		reads.range(b"m/", b"m0");
		assert!(store.commit(at, &WriteSet::new(), &reads).is_ok(), "a range nothing wrote into must still pass");
	}

	/// The headline claim, under concurrency: with transactions pinned at
	/// different stamps while the keyspace churns around them, every read is still
	/// the answer the full history would have given.
	///
	/// Each thread keeps every version it ever committed, which the store does
	/// not, and compares the store's answer against that. A collection that used
	/// the clock instead of the horizon, or dropped one version too many, shows up
	/// here as a read that disagrees with the history — silently, everywhere else.
	#[test]
	fn pinned_reads_match_the_full_history_under_concurrency() {
		const KEYS: usize = 8;
		const ROUNDS: usize = 100;
		let store = store();
		let mut handles = Vec::new();
		for key in 0..KEYS {
			let store = Arc::clone(&store);
			handles.push(std::thread::spawn(move || {
				let name = format!("k{key}");
				let mut history: Vec<(u64, Vec<u8>)> = Vec::new();
				for round in 0..ROUNDS {
					let pin = store.acquire().unwrap();
					let at = pin.stamp();
					let expected = history.iter().rev().find(|(stamp, _)| *stamp <= at).map(|(_, val)| val.clone());

					assert_eq!(
						store.get(name.as_bytes(), at).unwrap(),
						expected,
						"a pinned read disagreed with the full history"
					);

					let mut writes = WriteSet::new();
					writes.insert(name.clone().into_bytes(), Some(format!("{key}-{round}").into_bytes()));
					let stamp = store.commit(at, &writes, &ReadSet::new()).unwrap();
					history.push((stamp, format!("{key}-{round}").into_bytes()));

					// Other threads have been committing throughout, and this
					// transaction's own view has still not moved.
					assert_eq!(store.get(name.as_bytes(), at).unwrap(), expected, "a pinned snapshot moved");
				}
			}));
		}
		for handle in handles {
			handle.join().unwrap();
		}
		assert_eq!(pinned(&store), 0, "every pin was released when its thread finished");
	}

	/// A poisoned store serves nothing, ever. Recovering the guard and answering
	/// would mean answering from a keyspace whose versions and clock can disagree.
	#[test]
	fn a_poisoned_store_serves_nothing() {
		let store = store();
		write(&store, b"k", b"v");
		let at = store.snapshot().unwrap();

		// Panic with the lock held: that is what poisons a mutex.
		let hook = std::panic::take_hook();
		std::panic::set_hook(Box::new(|_| {}));
		let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
			let _held = store.lock().unwrap();
			panic!("poison the store");
		}));
		std::panic::set_hook(hook);
		assert!(panicked.is_err(), "the test has to actually poison the lock");

		assert!(matches!(store.get(b"k", at), Err(StorageError::Poisoned)));
		assert!(matches!(store.range(b"a", b"z", at), Err(StorageError::Poisoned)));
		assert!(matches!(store.acquire(), Err(StorageError::Poisoned)));
		assert!(matches!(store.collect(), Err(StorageError::Poisoned)));
		assert!(matches!(store.commit(at, &WriteSet::new(), &ReadSet::new()), Err(StorageError::Poisoned)));

		let mut writes = WriteSet::new();
		writes.insert(b"k".to_vec(), Some(b"later".to_vec()));
		assert!(matches!(store.commit(at, &writes, &ReadSet::new()), Err(StorageError::Poisoned)), "a poisoned store must not accept writes either");
	}
}