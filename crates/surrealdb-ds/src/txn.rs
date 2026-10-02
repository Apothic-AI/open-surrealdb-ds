//! `Transactable` — the transaction contract.
//!
//! Implemented against the **published** `surrealdb-kvs` 3.3.0 interface, read
//! from the crate source. See `../../docs/architecture.md` for the contract
//! map and `../../PROVENANCE.md` (class `PUBAPI`) for the audit trail.
//!
//! # ⚠ Phase 0 semantics are NOT the contract
//!
//! Writes land directly in the store on `set`, and `cancel` rolls nothing back.
//! A real transaction must stage writes and apply them atomically at commit.
//! That is Phase 2 work and it is the highest-risk area in the project: a subtly
//! wrong conflict rule produces silently wrong data rather than an error.
//!
//! The three properties that must eventually be right are marked inline and
//! tracked in `../../PROVENANCE.md`:
//!
//! - R-0010 `safe_timestamp` must be a genuine closed watermark
//! - R-0011 an indeterminate commit must be reported as such, not as failure
//! - R-0012 a write conflict must be retryable and distinguishable

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use surrealdb_kvs::api::{
	Batch, BoxFut, GetMultiResult, KeySpan, KeyValSpan, KeyVisitor, KeysBatch, KeysResult,
	ScanChunkStats, ScanCursorKeys, ScanCursorVals, ScanResult, Transactable, ValVisitor, ValsBatch,
};
use surrealdb_kvs::timestamp::{BoxTimeStamp, HlcTimeStamp};
use surrealdb_kvs::{Direction, Error, Key, KeyRange, Result, Val};

use crate::storage::MemoryStore;

/// A transaction over the engine's storage tier.
pub struct DsTxn {
	store: Arc<MemoryStore>,
	tx_type: TransactionKind,
	closed: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionKind {
	Read,
	Write,
}

impl DsTxn {
	pub(crate) fn begin(store: Arc<MemoryStore>, tx_type: surrealdb_kvs::TransactionType) -> Self {
		Self {
			store,
			tx_type: match tx_type {
				surrealdb_kvs::TransactionType::Read => TransactionKind::Read,
				surrealdb_kvs::TransactionType::Write => TransactionKind::Write,
			},
			closed: AtomicBool::new(false),
		}
	}

	fn read_guard(&self) -> Result<()> {
		if self.closed.load(Ordering::Acquire) {
			return Err(Error::TransactionFinished);
		}
		Ok(())
	}

	fn write_guard(&self) -> Result<()> {
		self.read_guard()?;
		if self.tx_type != TransactionKind::Write {
			return Err(Error::TransactionReadonly);
		}
		Ok(())
	}

	/// Forward range scan honouring `limit` and `skip`.
	fn scan_pairs(&self, range: &KeyRange<'_>) -> Vec<(Vec<u8>, Val)> {
		self.store.range(range.start.as_slice(), range.end.as_slice())
	}

	fn collect_pairs(
		&self,
		range: &KeyRange<'_>,
		limit: u32,
		skip: u32,
	) -> Vec<(Vec<u8>, Val)> {
		self.scan_pairs(range)
			.into_iter()
			.skip(skip as usize)
			.take(limit as usize)
			.collect()
	}

	fn reverse(pairs: Vec<(Vec<u8>, Val)>) -> Vec<(Vec<u8>, Val)> {
		let mut v = pairs;
		v.reverse();
		v
	}

	/// Build a `KeysResult`, filling the byte accounting the engine uses for
	/// planner estimates.
	fn keys_result(pairs: &[(Vec<u8>, Val)]) -> KeysResult {
		KeysResult {
			keys: pairs.iter().map(|(k, _)| k.clone()).collect(),
			key_bytes: pairs.iter().map(|(k, _)| k.len() as u64).sum(),
		}
	}

	fn scan_result(pairs: &[(Vec<u8>, Val)]) -> ScanResult {
		ScanResult {
			values: pairs.to_vec(),
			key_bytes: pairs.iter().map(|(k, _)| k.len() as u64).sum(),
			value_bytes: pairs.iter().map(|(_, v)| v.len() as u64).sum(),
		}
	}
}

impl Transactable for DsTxn {
	fn kind(&self) -> &'static str {
		"surrealds"
	}

	fn closed(&self) -> bool {
		self.closed.load(Ordering::Acquire)
	}

	fn writeable(&self) -> bool {
		self.tx_type == TransactionKind::Write
	}

	/// Discard the transaction.
	///
	/// Phase 2: must discard the staged write set. Nothing is staged yet, so
	/// this is honest rather than silently wrong — but it is not the contract.
	fn cancel(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.read_guard()?;
			self.closed.store(true, Ordering::Release);
			Ok(())
		})
	}

	fn commit(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			self.closed.store(true, Ordering::Release);
			Ok(())
		})
	}

	// ---- point operations -------------------------------------------------

	fn exists<'a>(&'a self, key: Key<'a>, _version: Option<u64>) -> BoxFut<'a, Result<bool>> {
		Box::pin(async move {
			self.read_guard()?;
			Ok(self.store.get(key.as_slice()).is_some())
		})
	}

	fn get<'a>(&'a self, key: Key<'a>, _version: Option<u64>) -> BoxFut<'a, Result<Option<Val>>> {
		Box::pin(async move {
			self.read_guard()?;
			Ok(self.store.get(key.as_slice()))
		})
	}

	/// Locked read (`SELECT … FOR UPDATE`).
	///
	/// The upstream default returns `Error::UnsupportedLockedReads`, which is the
	/// correct behaviour for a backend that cannot provide row locks. We can —
	/// Phase 2 registers read records for commit-time conflict detection. Until
	/// then, deferring is honest: claiming support we do not have would make
	/// `SELECT … FOR UPDATE` silently lose its conflict guarantee.
	fn getu<'a>(&'a self, _key: Key<'a>) -> BoxFut<'a, Result<Option<Val>>> {
		Box::pin(async move { Err(Error::UnsupportedLockedReads) })
	}

	fn set<'a>(&'a self, key: Key<'a>, val: Val) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			self.store.set(key.into_vec(), val);
			Ok(())
		})
	}

	fn put<'a>(&'a self, key: Key<'a>, val: Val) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			self.store.set(key.into_vec(), val);
			Ok(())
		})
	}

	/// Insert-or-update under a precondition.
	fn putc<'a>(&'a self, key: Key<'a>, val: Val, chk: Option<Val>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			let k = key.as_slice().to_vec();
			let current = self.store.get(&k);
			match (current, chk) {
				(None, None) => self.store.set(k, val),
				(Some(cur), Some(expected)) if cur == expected => self.store.set(k, val),
				// Precondition unmet, or a value was expected and none exists.
				_ => return Err(Error::TransactionConditionNotMet),
			}
			Ok(())
		})
	}

	fn del<'a>(&'a self, key: Key<'a>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			self.store.del(key.as_slice());
			Ok(())
		})
	}

	/// Delete under a precondition.
	fn delc<'a>(&'a self, key: Key<'a>, chk: Option<&'a [u8]>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			let k = key.as_slice().to_vec();
			match (self.store.get(&k), chk.map(<[u8]>::to_vec)) {
				(None, None) => {}
				(Some(cur), Some(expected)) if cur == expected => {
					self.store.del(&k);
				}
				_ => return Err(Error::TransactionConditionNotMet),
			}
			Ok(())
		})
	}

	fn replace<'a>(&'a self, key: Key<'a>, val: Val) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			self.store.set(key.into_vec(), val);
			Ok(())
		})
	}

	/// Delete all versions of a key.
	fn clr<'a>(&'a self, key: Key<'a>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			self.store.del(key.as_slice());
			Ok(())
		})
	}

	/// Delete all versions of a key under a precondition.
	fn clrc<'a>(&'a self, key: Key<'a>, chk: Option<&'a [u8]>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			let k = key.as_slice().to_vec();
			match (self.store.get(&k), chk.map(<[u8]>::to_vec)) {
				(None, None) => {}
				(Some(cur), Some(expected)) if cur == expected => {
					self.store.del(&k);
				}
				_ => return Err(Error::TransactionConditionNotMet),
			}
			Ok(())
		})
	}

	// ---- range operations -------------------------------------------------

	fn keys<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<KeysResult>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = self.collect_pairs(&rng, limit, skip);
			Ok(Self::keys_result(&pairs))
		})
	}

	fn keysr<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<KeysResult>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = Self::reverse(self.scan_pairs(&rng));
			let pairs: Vec<_> = pairs.into_iter().skip(skip as usize).take(limit as usize).collect();
			Ok(Self::keys_result(&pairs))
		})
	}

	fn scan<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<ScanResult>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = self.collect_pairs(&rng, limit, skip);
			Ok(Self::scan_result(&pairs))
		})
	}

	fn scanr<'a>(
		&'a self,
		rng: KeyRange<'a>,
		limit: u32,
		skip: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<ScanResult>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = Self::reverse(self.scan_pairs(&rng));
			let pairs: Vec<_> = pairs.into_iter().skip(skip as usize).take(limit as usize).collect();
			Ok(Self::scan_result(&pairs))
		})
	}

	fn getr<'a>(&'a self, rng: KeyRange<'a>, _version: Option<u64>) -> BoxFut<'a, Result<ScanResult>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = self.scan_pairs(&rng);
			Ok(Self::scan_result(&pairs))
		})
	}

	fn count<'a>(&'a self, rng: KeyRange<'a>, _version: Option<u64>) -> BoxFut<'a, Result<usize>> {
		Box::pin(async move {
			self.read_guard()?;
			Ok(self.scan_pairs(&rng).len())
		})
	}

	fn delr<'a>(&'a self, rng: KeyRange<'a>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move {
			self.write_guard()?;
			for (k, _) in self.scan_pairs(&rng) {
				self.store.del(&k);
			}
			Ok(())
		})
	}

	fn clrr<'a>(&'a self, rng: KeyRange<'a>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move { self.delr(rng).await })
	}

	// ---- multi-get --------------------------------------------------------

	/// Fetch many keys concurrently upstream; we walk them in order.
	///
	/// The result preserves input order with `None` for misses, and reports the
	/// found count and total value bytes — both feed planner estimates.
	fn getm<'a>(&'a self, keys: &'a [Key<'a>], _version: Option<u64>) -> BoxFut<'a, Result<GetMultiResult>> {
		Box::pin(async move {
			self.read_guard()?;
			let mut values = Vec::with_capacity(keys.len());
			let mut records = 0u64;
			let mut value_bytes = 0u64;
			for key in keys {
				match self.store.get(key.as_slice()) {
					Some(v) => {
						records += 1;
						value_bytes += v.len() as u64;
						values.push(Some(v));
					}
					None => values.push(None),
				}
			}
			Ok(GetMultiResult { values, records, value_bytes })
		})
	}

	// ---- batches ----------------------------------------------------------

	/// Partial batch scan: returns up to `batch` pairs plus the range to resume
	/// from, or `None` when the range is exhausted.
	fn batch_keys<'a>(
		&'a self,
		rng: KeyRange<'a>,
		batch: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<Batch<Vec<u8>>>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = self.scan_pairs(&rng);
			let window: Vec<Vec<u8>> = pairs.iter().map(|(k, _)| k.clone()).take(batch as usize).collect();
			let next = if pairs.len() > batch as usize {
				Some(KeyRange {
					start: Key::from(window.last().cloned().unwrap_or_default()),
					end: rng.end.clone().into_static(),
				})
			} else {
				None
			};
			Ok(Batch::new(next, window))
		})
	}

	fn batch_keys_vals<'a>(
		&'a self,
		rng: KeyRange<'a>,
		batch: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<Batch<(Vec<u8>, Val)>>> {
		Box::pin(async move {
			self.read_guard()?;
			let pairs = self.scan_pairs(&rng);
			let taken: Vec<(Vec<u8>, Val)> = pairs.iter().cloned().take(batch as usize).collect();
			let next = if pairs.len() > batch as usize {
				Some(KeyRange {
					start: Key::from(taken.last().map(|(k, _)| k.clone()).unwrap_or_default()),
					end: rng.end.clone().into_static(),
				})
			} else {
				None
			};
			let result = taken;
			Ok(Batch::new(next, result))
		})
	}

	// ---- savepoints -------------------------------------------------------

	fn new_save_point(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move { self.read_guard() })
	}

	fn release_last_save_point(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move { self.read_guard() })
	}

	/// Phase 2: must discard writes staged since the savepoint. Nothing is staged
	/// yet, so this is a no-op — flagged rather than silently wrong.
	fn rollback_to_save_point(&self) -> BoxFut<'_, Result<()>> {
		Box::pin(async move { self.read_guard() })
	}

	// ---- cursors ----------------------------------------------------------

	fn open_keys_cursor<'a>(
		&'a self,
		rng: KeyRange<'a>,
		dir: Direction,
		skip: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<Box<dyn ScanCursorKeys + 'a>>> {
		Box::pin(async move {
			self.read_guard()?;
			let mut pairs = self.scan_pairs(&rng);
			if dir == Direction::Backward {
				pairs.reverse();
			}
			let pairs: Vec<_> = pairs.into_iter().skip(skip as usize).collect();
			let cursor: Box<dyn ScanCursorKeys> = Box::new(KeysCursor::new(pairs));
			Ok(cursor)
		})
	}

	fn open_vals_cursor<'a>(
		&'a self,
		rng: KeyRange<'a>,
		dir: Direction,
		skip: u32,
		_version: Option<u64>,
	) -> BoxFut<'a, Result<Box<dyn ScanCursorVals + 'a>>> {
		Box::pin(async move {
			self.read_guard()?;
			let mut pairs = self.scan_pairs(&rng);
			if dir == Direction::Backward {
				pairs.reverse();
			}
			let pairs: Vec<_> = pairs.into_iter().skip(skip as usize).collect();
			let cursor: Box<dyn ScanCursorVals> = Box::new(ValsCursor::new(pairs));
			Ok(cursor)
		})
	}

	// ---- timestamps -------------------------------------------------------

	fn timestamp(&self) -> BoxFut<'_, Result<BoxTimeStamp>> {
		Box::pin(async move {
			self.read_guard()?;
			Ok(BoxTimeStamp::new(HlcTimeStamp::next()))
		})
	}

	/// R-0010: **must be overridden before this engine runs on more than one
	/// node.**
	///
	/// The default is correct only for a single monotonic oracle with synchronous
	/// local visibility. A multi-zone quorum commit log is non-linear — the
	/// highest committed timestamp can float above an unapplied lower one — so
	/// returning `timestamp()` here lets the live-query router advance its cursor
	/// past a commit that becomes visible later, and silently drop that
	/// notification.
	///
	/// Phase 3 implements a genuine closed watermark.
	fn safe_timestamp(&self) -> BoxFut<'_, Result<BoxTimeStamp>> {
		Box::pin(async move { self.timestamp().await })
	}

	/// Compaction hint. Phase 1 adds a real local engine with compaction; until
	/// then, declining is correct — silently succeeding would report work that
	/// never happened.
	fn compact<'a>(&'a self, _range: Option<KeyRange<'a>>) -> BoxFut<'a, Result<()>> {
		Box::pin(async move { Err(Error::CompactionNotSupported) })
	}
}

/// Zero-copy keys cursor over a materialised snapshot.
struct KeysCursor {
	pairs: Vec<(Vec<u8>, Val)>,
	pos: usize,
	buf: Vec<u8>,
	spans: Vec<KeySpan>,
}

impl KeysCursor {
	fn new(pairs: Vec<(Vec<u8>, Val)>) -> Self {
		Self { pairs, pos: 0, buf: Vec::new(), spans: Vec::new() }
	}
}

impl ScanCursorKeys for KeysCursor {
	fn next_batch<'s>(&'s mut self, limit: u32) -> BoxFut<'s, Result<KeysBatch<'s>>> {
		Box::pin(async move {
			// A zero limit is a pure no-op: it must not consume rows or exhaust
			// the cursor, and the cursor stays valid afterwards.
			if limit == 0 {
				return Ok(KeysBatch::from_parts(&[], &[], 0));
			}
			self.buf.clear();
			self.spans.clear();
			let mut key_bytes = 0u64;
			for (k, _) in self.pairs.iter().skip(self.pos).take(limit as usize) {
				self.spans.push(KeySpan { offset: self.buf.len(), len: k.len() });
				self.buf.extend_from_slice(k);
				key_bytes += k.len() as u64;
			}
			self.pos = (self.pos + limit as usize).min(self.pairs.len());
			Ok(KeysBatch::from_parts(&self.buf, &self.spans, key_bytes))
		})
	}

	fn for_each<'s>(
		&'s mut self,
		limit: u32,
		f: &'s mut dyn KeyVisitor,
	) -> BoxFut<'s, Result<ScanChunkStats>> {
		Box::pin(async move {
			let mut rows = 0u64;
			let mut key_bytes = 0u64;
			for (k, _) in self.pairs.iter().skip(self.pos).take(limit as usize) {
				// The visitor returns `Result<ControlFlow<()>, Error>`: a break is
				// a normal early stop, an error propagates.
				if f(k.as_slice())?.is_break() {
					break;
				}
				rows += 1;
				key_bytes += k.len() as u64;
			}
			self.pos = (self.pos + limit as usize).min(self.pairs.len());
			Ok(ScanChunkStats { rows, key_bytes, value_bytes: 0 })
		})
	}
}

/// Zero-copy `(key, value)` cursor over a materialised snapshot.
struct ValsCursor {
	pairs: Vec<(Vec<u8>, Val)>,
	pos: usize,
	key_buf: Vec<u8>,
	val_buf: Vec<u8>,
	spans: Vec<KeyValSpan>,
}

impl ValsCursor {
	fn new(pairs: Vec<(Vec<u8>, Val)>) -> Self {
		Self { pairs, pos: 0, key_buf: Vec::new(), val_buf: Vec::new(), spans: Vec::new() }
	}
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
			let mut key_bytes = 0u64;
			let mut value_bytes = 0u64;
			for (k, v) in self.pairs.iter().skip(self.pos).take(limit as usize) {
				self.spans.push(KeyValSpan {
					key_offset: self.key_buf.len(),
					key_len: k.len(),
					val_offset: self.val_buf.len(),
					val_len: v.len(),
				});
				self.key_buf.extend_from_slice(k);
				self.val_buf.extend_from_slice(v);
				key_bytes += k.len() as u64;
				value_bytes += v.len() as u64;
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
			let mut rows = 0u64;
			let mut key_bytes = 0u64;
			let mut value_bytes = 0u64;
			for (k, v) in self.pairs.iter().skip(self.pos).take(limit as usize) {
				if f(k.as_slice(), v.as_slice())?.is_break() {
					break;
				}
				rows += 1;
				key_bytes += k.len() as u64;
				value_bytes += v.len() as u64;
			}
			self.pos = (self.pos + limit as usize).min(self.pairs.len());
			Ok(ScanChunkStats { rows, key_bytes, value_bytes })
		})
	}
}