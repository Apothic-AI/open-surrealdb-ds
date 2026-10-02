//! `Transactable` — the transaction contract.
//!
//! Implemented against the **published** `surrealdb-kvs` 3.3.0 interface, read
//! from the crate source. See `../../docs/architecture.md` for the contract
//! map and `../../PROVENANCE.md` (class `PUBAPI`) for the audit trail.
//!
//! # What this transaction is
//!
//! Snapshot isolation with **read-set validation at commit**, staged writes, and
//! undo-log savepoints. Four properties do all the work:
//!
//! 1. **Reads see a stable snapshot.** A transaction captures a stamp when it
//!    begins and only ever reads the state as of that stamp, plus its own staged
//!    writes. Long-lived readers cannot be disturbed by concurrent writers.
//! 2. **Writes are staged.** `set` touches a private write set; nothing is
//!    visible to anyone, including this transaction's own point reads, until
//!    `commit`. `cancel` therefore discards by construction rather than by
//!    unwinding, and a commit that fails leaves nothing behind.
//! 3. **Every read is recorded, and validated at commit.** Snapshot isolation
//!    alone permits write skew; validating the recorded read set at commit is
//!    what removes it. A transaction that read a key nobody has since written
//!    commits; one that did not is refused with a retryable conflict.
//! 4. **A blind write does not conflict with another blind write.** Overlapping
//!    writes to one key all commit and serialise in stamp order. This is
//!    deliberate, and it is the opposite of first-committer-wins: the point of
//!    read-set validation is that the *reads* are checked, so a transaction that
//!    never read the key has nothing to be inconsistent with.
//!
//! # Threading
//!
//! One transaction is driven from one task, which is the contract upstream states
//! for the savepoint methods and which the rest of the trait inherits: the
//! operations keep state in `Mutex`es because their signatures take `&self`, not
//! because they are safe to interleave. A precondition check and the write that
//! depends on it are two separate steps, and nothing serialises them against each
//! other for the same key.
//!
//! Different transactions are fully independent: the only state they share is the
//! store, and a commit takes its lock once for both validation and application.
//!
//! The three properties that must eventually be right across *multiple nodes*
//! are tracked in `../../PROVENANCE.md`:
//!
//! - R-0010 `safe_timestamp` must be a genuine closed watermark
//! - R-0011 an indeterminate commit must be reported as such, not as failure
//! - R-0012 a write conflict must be retryable and distinguishable
//!
//! # Why `getu` is refused
//!
//! The trait's default rejects `getu` with `UnsupportedLockedReads` so that an
//! engine without a conflict-tracked read path cannot serve a read whose
//! guarantee would be silently discarded. This engine validates *every* read at
//! commit, which is a strictly stronger property than a locked read provides, so
//! there is no row lock to take — and `SELECT … FOR UPDATE` inherits the
//! validation that plain reads already get. Implementing a real lock registry
//! would add a weaker mechanism, not a missing one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use surrealdb_kvs::api::{
	BoxFut, GetMultiResult, KeySpan, KeyValSpan, KeyVisitor, KeysBatch, KeysResult, ScanChunkStats,
	ScanCursorKeys, ScanCursorVals, ScanResult, Transactable, ValVisitor, ValsBatch,
};
use surrealdb_kvs::timestamp::{BoxTimeStamp, HlcTimeStamp};
use surrealdb_kvs::{Direction, Error, Key, KeyRange, Result, Val};

use crate::storage::{ReadSet, VersionedStore, WriteSet};

/// A transaction over the engine's storage tier.
pub struct DsTxn {
	store: Arc<VersionedStore>,
	tx_type: TransactionKind,
	/// The stamp this transaction reads at. Every read is resolved against it, so
	/// the view cannot move under a live transaction.
	snapshot: u64,
	/// Writes staged for commit. `None` is a delete. Empty for a read-only
	/// transaction, which never stages anything.
	writes: Mutex<WriteSet>,
	/// Everything this transaction has read, for commit-time validation.
	reads: Mutex<ReadSet>,
	/// Open savepoint scopes, innermost last.
	savepoints: Mutex<Vec<UndoScope>>,
	closed: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionKind {
	Read,
	Write,
}

/// The undo log of one open savepoint scope.
///
/// An entry is the value the key held when this scope first touched it — which
/// may be a value this transaction staged earlier, not just a committed one, so
/// rollback restores the transaction's own view and not the snapshot's.
#[derive(Debug, Default)]
struct UndoScope {
	undo: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl DsTxn {
	pub(crate) fn begin(store: Arc<VersionedStore>, tx_type: surrealdb_kvs::TransactionType) -> Self {
		let snapshot = store.snapshot();
		Self {
			store,
			tx_type: match tx_type {
				surrealdb_kvs::TransactionType::Read => TransactionKind::Read,
				surrealdb_kvs::TransactionType::Write => TransactionKind::Write,
			},
			snapshot,
			writes: Mutex::new(WriteSet::new()),
			reads: Mutex::new(ReadSet::new()),
			savepoints: Mutex::new(Vec::new()),
			closed: AtomicBool::new(false),
		}
	}

	fn lock<'a, T>(m: &'a Mutex<T>) -> MutexGuard<'a, T> {
		m.lock().unwrap_or_else(|e| e.into_inner())
	}

	fn open(&self) -> bool {
		!self.closed.load(Ordering::Acquire)
	}

	fn require_open(&self) -> Result<()> {
		if self.open() {
			Ok(())
		} else {
			Err(Error::TransactionFinished)
		}
	}

	/// Reject a read that names a version: this engine keeps history for snapshot
	/// isolation, not for time travel, and answering such a read from the
	/// wrong version would be worse than refusing it.
	fn reject_version(version: Option<u64>) -> Result<()> {
		match version {
			None => Ok(()),
			Some(_) => Err(Error::UnsupportedVersionedQueries),
		}
	}

	fn require_writable(&self) -> Result<()> {
		self.require_open()?;
		if self.tx_type == TransactionKind::Write {
			Ok(())
		} else {
			Err(Error::TransactionReadonly)
		}
	}

	/// The live value of `key` to this transaction: its staged value if it has
	/// been written, otherwise its value at the snapshot.
	///
	/// A staged delete reads as absent, which is what makes `put` legal on a key
	/// this transaction has just deleted, and what makes `delc(key, None)` a
	/// check for absence rather than a check for "was never written".
	///
	/// Returning owned bytes keeps the store lock from having to outlive the
	/// answer; at Phase 0 volumes that is the right trade.
	fn effective(&self, key: &[u8], writes: &WriteSet) -> Option<Vec<u8>> {
		match writes.get(key) {
			Some(val) => val.clone(),
			None => self.store.get(key, self.snapshot),
		}
	}

	/// Record a point read of `key`.
	fn record_point(&self, key: &[u8]) {
		Self::lock(&self.reads).point(key);
	}

	/// Record a read of `[start, end)`.
	fn record_range(&self, start: &[u8], end: &[u8]) {
		Self::lock(&self.reads).range(start, end);
	}

	/// Stage a write, recording the pre-image in every open scope that has not
	/// already recorded this key.
	///
	/// The innermost scope owns the pre-image and the enclosing ones must not
	/// overwrite it with a later one: rolling back to an enclosing scope has to
	/// restore the value as of *that* scope, not as of a nested one.
	fn stage_write(&self, key: Vec<u8>, val: Option<Vec<u8>>) {
		let before = self.effective(&key, &Self::lock(&self.writes));
		// The two locks are never held together, in either order: a rollback
		// reads the write set while holding the savepoint stack, so taking them
		// in one order here and the other there would be a deadlock.
		let mut scopes = Self::lock(&self.savepoints);
		for scope in scopes.iter_mut() {
			scope.undo.entry(key.clone()).or_insert_with(|| before.clone());
		}
		drop(scopes);
		Self::lock(&self.writes).insert(key, val);
	}

	/// `keys` / `keysr`: recorded keys in `rng`, honouring `limit` and `skip`.
	fn collect_keys(
		&self,
		rng: &KeyRange<'_>,
		limit: u32,
		skip: u32,
		reverse: bool,
	) -> Vec<Vec<u8>> {
		self.record_range(rng.start.as_slice(), rng.end.as_slice());
		let writes = Self::lock(&self.writes);
		let mut pairs = self.scan_pairs(rng, &writes);
		if reverse {
			pairs.reverse();
		}
		pairs.into_iter().skip(skip as usize).take(limit as usize).map(|(k, _)| k).collect()
	}

	/// `scan` / `scanr` / `getr`: recorded pairs in `rng`, honouring `limit` and
	/// `skip`.
	fn collect_pairs(
		&self,
		rng: &KeyRange<'_>,
		limit: u32,
		skip: u32,
		reverse: bool,
	) -> Vec<(Vec<u8>, Val)> {
		self.record_range(rng.start.as_slice(), rng.end.as_slice());
		let writes = Self::lock(&self.writes);
		let mut pairs = self.scan_pairs(rng, &writes);
		if reverse {
			pairs.reverse();
		}
		pairs.into_iter().skip(skip as usize).take(limit as usize).collect()
	}

	/// The transaction's view of `[start, end)`: the snapshot's pairs with the
	/// staged writes for that span merged over them.
	fn scan_pairs(&self, rng: &KeyRange<'_>, writes: &WriteSet) -> Vec<(Vec<u8>, Val)> {
		let start = rng.start.as_slice();
		let end = rng.end.as_slice();
		let mut merged: BTreeMap<Vec<u8>, Val> =
			self.store.range(start, end, self.snapshot).into_iter().collect();
		for (key, val) in writes.range(start.to_vec()..).take_while(|(k, _)| k.as_slice() < end) {
			match val {
				Some(val) => {
					merged.insert(key.clone(), val.clone());
				}
				None => {
					merged.remove(key);
				}
			}
		}
		merged.into_iter().collect()
	}

	/// Open a savepoint scope.
	fn push_savepoint(&self) {
		Self::lock(&self.savepoints).push(UndoScope::default());
	}

	/// Close the innermost scope, keeping its writes.
	///
	/// Its undo log transfers to the enclosing scope rather than being dropped:
	/// a released scope's writes stay committed, but they must still be undone
	/// by a rollback of a scope that encloses them. Releasing the outermost scope
	/// therefore closes the stack — there is nothing left to roll back to.
	fn release_savepoint(&self) {
		let mut scopes = Self::lock(&self.savepoints);
		let Some(inner) = scopes.pop() else {
			// Releasing with nothing open is accepted and inert.
			return;
		};
		if let Some(outer) = scopes.last_mut() {
			for (key, before) in inner.undo {
				outer.undo.entry(key).or_insert(before);
			}
		}
	}

	/// Undo everything written since the innermost scope was opened, leaving any
	/// enclosing scope open and still able to undo its own writes.
	///
	/// A restored value that came from the snapshot rather than from this
	/// transaction's own writes is materialised into the write set, so the
	/// transaction will re-write bytes that did not change. That costs an extra
	/// version per restored key and is otherwise invisible: the write set is
	/// never conflict-checked, and the committed bytes are the same.
	fn rollback_savepoint(&self) -> Result<()> {
		let Some(scope) = Self::lock(&self.savepoints).pop() else {
			return Err(Error::NoSavepoint);
		};
		let mut writes = Self::lock(&self.writes);
		for (key, before) in scope.undo {
			match before {
				Some(val) => {
					writes.insert(key, Some(val));
				}
				None => {
					writes.remove(&key);
				}
			}
		}
		Ok(())
	}

	/// Check a conditional write/delete against the value the condition was
	/// written against.
	///
	/// `None` as the expected value is a claim that the key does *not* exist,
	/// not a claim that anything goes — that is what makes a conditional create
	/// atomic, and it is why `putc`/`delc` register a read (see [`Self::record_point`]).
	fn check(&self, key: &[u8], expected: Option<&[u8]>) -> Result<()> {
		self.record_point(key);
		let writes = Self::lock(&self.writes);
		match (self.effective(key, &writes).as_deref(), expected) {
			(None, None) => Ok(()),
			(Some(actual), Some(expected)) if actual == expected => Ok(()),
			_ => Err(Error::TransactionConditionNotMet),
		}
	}
}

impl Transactable for DsTxn {
	fn kind(&self) -> &'static str {
		"surrealds"
	}

	fn closed(&self) -> bool {
		!self.open()
	}

	fn writeable(&self) -> bool {
		self.tx_type == TransactionKind::Write
	}

	/// Discard the transaction.
	///
	/// Nothing was applied — writes live in this transaction's own write set
	/// until `commit` — so discarding is closing. A savepoint's undo log goes
	/// with it: nothing about the scope outlives the transaction that opened it.
	fn cancel(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.require_open()?;
			self.store.note_cancel();
			Self::lock(&self.savepoints).clear();
			Self::lock(&self.writes).clear();
			self.closed.store(true, Ordering::Release);
			Ok(())
		})
	}

	/// Make the staged writes visible, atomically.
	///
	/// The recorded read set is validated first: if any key this transaction read
	/// was written since its snapshot, the commit is refused with
	/// [`Error::TransactionConflict`], which is retryable and must be retried
	/// from the beginning. Blind writes to a key another transaction is also
	/// writing are not a conflict — they serialise in stamp order.
	///
	/// A read-only transaction has nothing to commit, so committing one succeeds
	/// and closes it; it validates nothing, because there is no write that could
	/// follow from its reads.
	fn commit(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.require_open()?;
			let writes = Self::lock(&self.writes);
			if self.tx_type == TransactionKind::Write {
				let reads = Self::lock(&self.reads);
				if let Err(conflict) = self.store.commit(self.snapshot, &writes, &reads) {
					return Err(Error::TransactionConflict(format!(
						"{} key(s) changed since the transaction's snapshot",
						conflict.keys.len()
					)));
				}
			}
			drop(writes);
			self.closed.store(true, Ordering::Release);
			Ok(())
		})
	}

	// ---- point reads -------------------------------------------------------

	fn exists<'a>(&'a self, key: Key<'a>, version: Option<u64>) -> BoxFut<'a, Result<bool>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			self.record_point(key.as_slice());
			let writes = Self::lock(&self.writes);
			Ok(self.effective(key.as_slice(), &writes).is_some())
		})
	}

	fn get<'a>(&'a self, key: Key<'a>, version: Option<u64>) -> BoxFut<'a, Result<Option<Val>>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			self.record_point(key.as_slice());
			let writes = Self::lock(&self.writes);
			Ok(self.effective(key.as_slice(), &writes))
		})
	}

	/// Locked read (`SELECT … FOR UPDATE`).
	///
	/// Refused with `UnsupportedLockedReads`. Every read this engine performs is
	/// validated against the transaction's snapshot at commit, which already
	/// gives `SELECT … FOR UPDATE` its conflict guarantee; see the module docs.
	fn getu<'a>(&'a self, _key: Key<'a>) -> BoxFut<'a, Result<Option<Val>>> {
		Box::pin(async move { Err(Error::UnsupportedLockedReads) })
	}

	fn getm<'a>(&'a self, keys: &'a [Key<'a>], version: Option<u64>) -> BoxFut<'a, Result<GetMultiResult>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			let mut values = Vec::with_capacity(keys.len());
			let mut records = 0u64;
			let mut value_bytes = 0u64;
			let writes = Self::lock(&self.writes);
			for key in keys {
				self.record_point(key.as_slice());
				match self.effective(key.as_slice(), &writes) {
					Some(val) => {
						records += 1;
						value_bytes += val.len() as u64;
						values.push(Some(val));
					}
					// Staged as deleted, or absent from the snapshot.
					None => values.push(None),
				}
			}
			Ok(GetMultiResult { values, records, value_bytes })
		})
	}

	// ---- point writes ------------------------------------------------------

	fn set<'a>(&'a self, key: Key<'a>, val: Val) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.require_writable()?;
			self.stage_write(key.into_vec(), Some(val));
			Ok(())
		})
	}

	/// Insert, refusing a key that already exists to this transaction.
	fn put<'a>(&'a self, key: Key<'a>, val: Val) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.require_writable()?;
			self.check(key.as_slice(), None)?;
			self.stage_write(key.into_vec(), Some(val));
			Ok(())
		})
	}

	/// Insert-or-update under a precondition. See [`Self::check`].
	fn putc<'a>(&'a self, key: Key<'a>, val: Val, chk: Option<Val>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.require_writable()?;
			self.check(key.as_slice(), chk.as_deref())?;
			self.stage_write(key.into_vec(), Some(val));
			Ok(())
		})
	}

	/// Delete. Deleting an absent key is not an error, and is not a no-op either:
	/// it is a write, so a transaction that read the key must be told about it.
	fn del<'a>(&'a self, key: Key<'a>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.require_writable()?;
			self.stage_write(key.into_vec(), None);
			Ok(())
		})
	}

	/// Delete under a precondition. See [`Self::check`].
	fn delc<'a>(&'a self, key: Key<'a>, chk: Option<&'a [u8]>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.require_writable()?;
			self.check(key.as_slice(), chk)?;
			self.stage_write(key.into_vec(), None);
			Ok(())
		})
	}

	// ---- range reads -------------------------------------------------------

	fn keys<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		version: Option<u64>,
	) -> BoxFut<'a, Result<KeysResult>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			let keys = self.collect_keys(&rng, limit, skip, false);
			Ok(KeysResult { key_bytes: keys.iter().map(|k| k.len() as u64).sum(), keys })
		})
	}

	fn keysr<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		version: Option<u64>,
	) -> BoxFut<'a, Result<KeysResult>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			let keys = self.collect_keys(&rng, limit, skip, true);
			Ok(KeysResult { key_bytes: keys.iter().map(|k| k.len() as u64).sum(), keys })
		})
	}

	fn scan<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		version: Option<u64>,
	) -> BoxFut<'a, Result<ScanResult>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			Ok(scan_result(&self.collect_pairs(&rng, limit, skip, false)))
		})
	}

	fn scanr<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		version: Option<u64>,
	) -> BoxFut<'a, Result<ScanResult>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			Ok(scan_result(&self.collect_pairs(&rng, limit, skip, true)))
		})
	}

	fn getr<'a>(&'a self, rng: KeyRange<'a>, version: Option<u64>) -> BoxFut<'a, Result<ScanResult>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			Ok(scan_result(&self.collect_pairs(&rng, u32::MAX, 0, false)))
		})
	}

	fn count<'a>(&'a self, rng: KeyRange<'a>, version: Option<u64>) -> BoxFut<'a, Result<usize>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			self.record_range(rng.start.as_slice(), rng.end.as_slice());
			let writes = Self::lock(&self.writes);
			Ok(self.scan_pairs(&rng, &writes).len())
		})
	}

	// ---- savepoints --------------------------------------------------------

	fn new_save_point(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.require_open()?;
			self.push_savepoint();
			Ok(())
		})
	}

	/// Close the innermost scope, keeping its writes but leaving them undoable by
	/// an enclosing scope. Releasing with no scope open is accepted and inert.
	fn release_last_save_point(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.require_open()?;
			self.release_savepoint();
			Ok(())
		})
	}

	/// Undo everything written since the innermost scope was opened.
	fn rollback_to_save_point(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.require_open()?;
			self.rollback_savepoint()
		})
	}

	// ---- cursors -----------------------------------------------------------

	/// A keys cursor over the transaction's view of `rng`.
	///
	/// The range is materialised when the cursor is opened, not read through as
	/// it is pumped: a cursor is one logical scan that must see a consistent
	/// prefix of a stable snapshot, and re-resolving each batch against a
	/// snapshot that a concurrent writer cannot move would buy nothing. The cost
	/// is that a cursor over a large range holds it in memory — the thing to fix
	/// when a real local engine with a native iterator replaces the tier.
	fn open_keys_cursor<'a>(
		&'a self,
		rng: KeyRange<'a>,
		dir: Direction,
		skip: u32,
		version: Option<u64>,
	) -> BoxFut<'a, Result<Box<dyn ScanCursorKeys + 'a>>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			let keys = self.collect_keys(&rng, u32::MAX, skip, dir == Direction::Backward);
			Ok(Box::new(KeysCursor { keys, pos: 0, buf: Vec::new(), spans: Vec::new() })
				as Box<dyn ScanCursorKeys + 'a>)
		})
	}

	/// A `(key, value)` cursor over the transaction's view of `rng`. See
	/// [`Self::open_keys_cursor`] on materialisation.
	fn open_vals_cursor<'a>(
		&'a self,
		rng: KeyRange<'a>,
		dir: Direction,
		skip: u32,
		version: Option<u64>,
	) -> BoxFut<'a, Result<Box<dyn ScanCursorVals + 'a>>> {
		Box::pin(async move {
			self.require_open()?;
			Self::reject_version(version)?;
			let pairs = self.collect_pairs(&rng, u32::MAX, skip, dir == Direction::Backward);
			Ok(Box::new(ValsCursor {
				pairs,
				pos: 0,
				key_buf: Vec::new(),
				val_buf: Vec::new(),
				spans: Vec::new(),
			}) as Box<dyn ScanCursorVals + 'a>)
		})
	}

	// ---- timestamps --------------------------------------------------------

	/// The engine's clock.
	///
	/// `HlcTimeStamp::next` is strictly increasing across the process, so it is
	/// usable as the read version of a committed write. It is *not* a cluster
	/// clock: see [`Self::safe_timestamp`].
	fn timestamp(&self) -> BoxFut<'_, Result<BoxTimeStamp>> {
		Box::pin(async move {
			self.require_open()?;
			Ok(BoxTimeStamp::new(HlcTimeStamp::next()))
		})
	}

	/// R-0010: **must be replaced by a genuine closed watermark before this
	/// engine runs on more than one node.**
	///
	/// The default is correct only for a single monotonic oracle with synchronous
	/// local visibility. A quorum commit log is non-linear — the highest
	/// committed stamp can float above one that is committed but not yet applied
	/// here — so returning `timestamp()` would let the live-query router advance
	/// its cursor past a commit that becomes visible later, silently dropping
	/// that notification.
	///
	/// Single-node, every commit is applied before the next stamp is handed out,
	/// so this is correct as written today and wrong the moment a peer exists.
	fn safe_timestamp(&self) -> BoxFut<'_, Result<BoxTimeStamp>> {
		Box::pin(async move { self.timestamp().await })
	}

	/// Compaction hint, declined.
	///
	/// The in-memory tier reclaims nothing, so there is no honest answer but
	/// "not supported". Phase 1 gives this a local engine that compacts; a
	/// success reported over a no-op would tell an operator space was reclaimed
	/// that was not.
	fn compact<'a>(&'a self, _range: Option<KeyRange<'a>>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move { Err(Error::CompactionNotSupported) })
	}
}

/// Fill in the byte accounting the engine uses for planner estimates.
fn scan_result(pairs: &[(Vec<u8>, Val)]) -> ScanResult {
	ScanResult {
		key_bytes: pairs.iter().map(|(k, _)| k.len() as u64).sum(),
		value_bytes: pairs.iter().map(|(_, v)| v.len() as u64).sum(),
		values: pairs.to_vec(),
	}
}

/// Zero-copy keys cursor over a materialised snapshot of the range.
struct KeysCursor {
	keys: Vec<Vec<u8>>,
	pos: usize,
	buf: Vec<u8>,
	spans: Vec<KeySpan>,
}

impl ScanCursorKeys for KeysCursor {
	fn next_batch<'s>(&'s mut self, limit: u32) -> BoxFut<'s, Result<KeysBatch<'s>>> {
		Box::pin(async move {
			// A zero limit is a pure no-op: it must not consume rows, must not
			// exhaust the cursor, and must leave the cursor usable.
			if limit == 0 {
				return Ok(KeysBatch::from_parts(&[], &[], 0));
			}
			self.buf.clear();
			self.spans.clear();
			let mut key_bytes = 0u64;
			for key in self.keys.iter().skip(self.pos).take(limit as usize) {
				self.spans.push(KeySpan {
					offset: self.buf.len(),
					len: key.len(),
				});
				self.buf.extend_from_slice(key);
				key_bytes += key.len() as u64;
			}
			self.pos = (self.pos + limit as usize).min(self.keys.len());
			Ok(KeysBatch::from_parts(&self.buf, &self.spans, key_bytes))
		})
	}

	fn for_each<'s>(&'s mut self, limit: u32, f: &'s mut dyn KeyVisitor) -> BoxFut<'s, Result<ScanChunkStats>> {
		Box::pin(async move {
			if limit == 0 {
				return Ok(ScanChunkStats::default());
			}
			let (mut rows, mut key_bytes) = (0u64, 0u64);
			for key in self.keys.iter().skip(self.pos).take(limit as usize) {
				// The visitor returns `Result<ControlFlow<()>, Error>`: a break is
				// a normal early stop, an error propagates. A broken row was
				// still visited and must still be consumed, so the next call
				// resumes after it rather than repeating it.
				let stop = f(key)?.is_break();
				rows += 1;
				key_bytes += key.len() as u64;
				self.pos += 1;
				if stop {
					break;
				}
			}
			Ok(ScanChunkStats { rows, key_bytes, value_bytes: 0 })
		})
	}
}

/// Zero-copy `(key, value)` cursor over a materialised snapshot of the range.
struct ValsCursor {
	pairs: Vec<(Vec<u8>, Val)>,
	pos: usize,
	key_buf: Vec<u8>,
	val_buf: Vec<u8>,
	spans: Vec<KeyValSpan>,
}

impl ScanCursorVals for ValsCursor {
	fn next_batch<'s>(&'s mut self, limit: u32) -> BoxFut<'s, Result<ValsBatch<'s>>> {
		Box::pin(async move {
			if limit == 0 {
				return Ok(ValsBatch::from_parts(&[], &[], &[], 0, 0));
			}
			self.key_buf.clear();
			self.val_buf.clear();
			self.spans.clear();
			let (mut key_bytes, mut value_bytes) = (0u64, 0u64);
			for (key, val) in self.pairs.iter().skip(self.pos).take(limit as usize) {
				self.spans.push(KeyValSpan {
					key_offset: self.key_buf.len(),
					key_len: key.len(),
					val_offset: self.val_buf.len(),
					val_len: val.len(),
				});
				self.key_buf.extend_from_slice(key);
				self.val_buf.extend_from_slice(val);
				key_bytes += key.len() as u64;
				value_bytes += val.len() as u64;
			}
			self.pos = (self.pos + limit as usize).min(self.pairs.len());
			Ok(ValsBatch::from_parts(&self.key_buf, &self.val_buf, &self.spans, key_bytes, value_bytes))
		})
	}

	fn for_each<'s>(
		&'s mut self,
		limit: u32,
		f: &'s mut dyn ValVisitor,
	) -> BoxFut<'s, Result<ScanChunkStats>> {
		Box::pin(async move {
			if limit == 0 {
				return Ok(ScanChunkStats::default());
			}
			let (mut rows, mut key_bytes, mut value_bytes) = (0u64, 0u64, 0u64);
			for (key, val) in self.pairs.iter().skip(self.pos).take(limit as usize) {
				let stop = f(key, val)?.is_break();
				rows += 1;
				key_bytes += key.len() as u64;
				value_bytes += val.len() as u64;
				self.pos += 1;
				if stop {
					break;
				}
			}
			Ok(ScanChunkStats { rows, key_bytes, value_bytes })
		})
	}
}

