//! In-memory storage tier.
//!
//! Deliberately holds **no SurrealDB imports** — this module is the Phase 1/4
//! seam where a real local engine and an object-storage durable tier get
//! substituted. See ADR-0002.
//!
//! Phase 0 semantics: a `BTreeMap` per store. No persistence, no concurrency
//! control, no snapshot isolation. Correctness work starts in Phase 1.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// An ordered keyspace. `BTreeMap` gives us the range and ordering semantics
/// `KeyRange` scans rely on, which is the main reason not to reach for a hash
/// map here.
#[derive(Debug, Default)]
pub struct MemoryStore {
	inner: Mutex<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl MemoryStore {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
		self.inner.lock().expect("store poisoned").get(key).cloned()
	}

	pub fn set(&self, key: Vec<u8>, val: Vec<u8>) {
		self.inner.lock().expect("store poisoned").insert(key, val);
	}

	pub fn del(&self, key: &[u8]) -> bool {
		self.inner.lock().expect("store poisoned").remove(key).is_some()
	}

	/// Pairs in `[start, end)` — half-open, matching `KeyRange`'s exclusive
	/// upper bound. An empty `start`/`end` means unbounded on that side.
	pub fn range(&self, start: &[u8], end: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
		let guard = self.inner.lock().expect("store poisoned");
		let out: Vec<(Vec<u8>, Vec<u8>)> = guard
			.range(start.to_vec()..)
			// `KeyRange`'s upper bound is exclusive.
			.take_while(|(k, _)| k.as_slice() < end)
			.map(|(k, v)| (k.clone(), v.clone()))
			.collect();
		out
	}

	pub fn count(&self) -> usize {
		self.inner.lock().expect("store poisoned").len()
	}

	pub fn clear(&self) {
		self.inner.lock().expect("store poisoned").clear();
	}
}