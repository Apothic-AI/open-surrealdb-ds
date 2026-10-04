//! Shared by the byte-compatibility harnesses: the dataset, the scratch space,
//! and the one way bytes move between tiers.
//!
//! Split out of `golden.rs` unchanged so that `interop.rs` authors and dumps the
//! *same* dataset rather than a second, subtly different one. Two harnesses that
//! each carried their own copy of `WORKLOAD` would drift, and the day they
//! disagreed the cheaper one would quietly become a test of nothing.
//!
//! Nothing here knows about a golden file or about RocksDB. What moves between
//! tiers is always `(key bytes, value bytes)`, and that is the whole contract
//! this project's harnesses check.
//!
//! `dead_code` is allowed in this module, deliberately: it is compiled into two
//! test targets and each uses a different part of it. Lint runs per crate, so
//! the half the other target does not touch would otherwise warn, and
//! duplicating the code to silence that would defeat the split.
//!
//! # Licence
//!
//! BUSL-1.1, like the crate it lives in: it links `surrealdb-server`.
//! See ../NOTICE and DECISIONS.md ADR-0002.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use surrealdb_cnf::ConfigMap;
use surrealdb_ds_server::{DsComposer, registry};
use surrealdb_kvs::key::{AnyRange, RawRange};
use surrealdb_kvs::{Key, KeyRange, Transactable, TransactionBuilder, TransactionType};
use surrealdb_server::core::dbs::Session;
use surrealdb_server::core::kvs::{Builder, Datastore};
use tokio_util::sync::CancellationToken;

// ─────────────────────────────────────────────────────────────────────────────
// The dataset
// ─────────────────────────────────────────────────────────────────────────────

/// The SurrealQL workload that authors the golden dataset.
///
/// Chosen to make the dataset non-trivial in the ways Phase 1's exit criterion
/// names, and to reach key classes with genuinely awkward encodings:
///
/// | Statement | Key classes it produces |
/// | --- | --- |
/// | `DEFINE TABLE … PERMISSIONS` | `TableKey` — the serialised `StoredPermissions`, plus five `Uuid::new_v7()` cache watermarks that make the whole value irreproducible |
/// | `DEFINE FIELD … TYPE array<float, 3>` | `FieldKey` — a field definition and its per-column sub-definition |
/// | `DEFINE INDEX … UNIQUE COLUMNS` | `IndexDefKey`, `IndexNameKey`, and the `+{ix}` index entries holding `IndexFormat`-encoded values |
/// | `DEFINE INDEX … HNSW DIMENSION 3` | `IndexDefKey` for a vector index, plus the `!hr` pending-update records that carry the serialised vectors |
/// | `CREATE person:one/two/three` | the record documents themselves, the `!di`/`!dj`/`!dd` doc-id mappings, `!bt` doc-id batches |
/// | `RELATE … SET id = …` | graph edges: the `~` in/out edge keys on **both** endpoints and the edge document |
/// | `UPDATE person:one SET …` | a second write to a key that already exists — see the version-history leg for why that is checked separately |
/// | `DELETE person:three` | a tombstone, on a record of its own so the graph edges and index entries built from the other two survive it |
///
/// Explicit edge ids (`SET id = "edge_one"`) rather than generated ones: a
/// generated graph edge id is a fresh ULID per run, which would make every key
/// touching the edge irreproducible and throw away the graph half of the
/// dataset.
pub const WORKLOAD: &str = r#"
DEFINE NAMESPACE golden;
USE NS golden;
DEFINE DATABASE golden;
USE DB golden;

DEFINE TABLE person SCHEMALESS PERMISSIONS FOR select, update, delete WHERE user = $auth.id;

DEFINE FIELD name ON person TYPE string;
DEFINE FIELD embedding ON person TYPE array<float, 3>;
DEFINE INDEX person_email ON person COLUMNS email UNIQUE;
DEFINE INDEX person_vec ON person FIELDS embedding HNSW DIMENSION 3 DIST EUCLIDEAN;

CREATE person:one SET name = "one", email = "one@example.com", embedding = [1.0, 2.0, 3.0];
CREATE person:two SET name = "two", email = "two@example.com", embedding = [4.0, 5.0, 6.0];
CREATE person:three SET name = "three", email = "three@example.com", embedding = [7.0, 8.0, 9.0];

-- A key written twice: the version list behind this key has two entries.
UPDATE person:one SET name = "one revised";

RELATE person:one->knows->person:two SET id = "edge_one", since = 2020;
RELATE person:two->knows->person:one SET id = "edge_two", since = 2021;

-- A tombstone on a record of its own, so the graph edges and index entries built
-- from `person:one` and `person:two` survive it and stay in the dataset.
DELETE person:three;
"#;

/// Batches per replay transaction.
///
/// Not load-bearing for correctness — one transaction per batch and one giant
/// transaction both have to produce the same bytes — but a single transaction
/// over a whole dataset is a shape no real writer uses, and the harness should
/// not test a shape the query layer never produces.
pub const REPLAY_BATCH: usize = 64;

/// Rows per scan page when dumping a keyspace.
pub const PAGE: u32 = 1024;

/// Our own engine, by the scheme this project registers.
pub const OURS: &str = "ds+mem://";

/// The key the version-history leg rewrites: shaped like a record key under the
/// `person` table, so it sorts and encodes the way the rest of the dataset does.
pub const VERSIONED_KEY: &[u8] = b"/\x00\x00\x00\x00\x00\x00\x00\x00*person\x00*revived\x00";

/// A whole keyspace, in the order it is compared in.
///
/// `BTreeMap` rather than `Vec` so two dumps are compared as keyspaces rather
/// than as sequences: a backend that returned the right pairs in a different
/// order would still be correct, and the harness must not fail it for that.
pub type Keyspace = BTreeMap<Vec<u8>, Vec<u8>>;

/// A `KeyRange` over raw byte bounds.
///
/// `RawRange::of_bytes` is the storage layer's own "these bytes are not a
/// declared key" constructor, which is exactly what a harness comparing two
/// engines' whole keyspaces needs: it has no declared bound to start from.
pub fn bytes(start: &[u8], end: &[u8]) -> KeyRange<'static> {
	RawRange::of_bytes(start, end).into_key_range()
}

pub fn hex(bytes: &[u8]) -> String {
	let mut out = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		let _ = write!(out, "{byte:02x}");
	}
	out
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
	if !text.len().is_multiple_of(2) {
		return None;
	}
	(0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// A key or a value as something a human can read: escaped ASCII where it is
/// ASCII, hex otherwise. A diff of raw hex tells you two keys differ and nothing
/// else.
pub fn show(bytes: &[u8]) -> String {
	let printable = bytes.iter().all(|b| (0x20..0x7f).contains(b));
	if printable {
		format!("{:?} ({})", String::from_utf8_lossy(bytes), hex(bytes))
	} else {
		hex(bytes)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Talking to a backend
// ─────────────────────────────────────────────────────────────────────────────

/// One open backend, reached through the same registry the binary serves from.
///
/// A type rather than free functions over a path string, because
/// **`new_transaction_builder` is what opens the store**: `ds+mem://` builds a
/// fresh in-memory tier per call, so "write to `ds+mem://`, then scan
/// `ds+mem://`" as two calls would compare an empty store against a populated one
/// and call it a mismatch. Holding the builder for the life of the backend keeps
/// every transaction on the same store.
pub struct Backend {
	builder: Box<dyn TransactionBuilder>,
}

impl Backend {
	/// Open the backend a datastore path names — upstream's `rocksdb:` or our
	/// `ds+mem://`, through one code path so the comparison is not two.
	pub async fn open(path: &str) -> anyhow::Result<Self> {
		let builder =
			registry().new_transaction_builder(path, CancellationToken::new(), ConfigMap::default()).await?;
		Ok(Self { builder })
	}

	pub async fn txn(&self, kind: TransactionType) -> anyhow::Result<Box<dyn Transactable>> {
		Ok(self.builder.new_transaction(kind).await?.0)
	}

	/// Write every pair in, in ordinary batched transactions, then read the whole
	/// keyspace back.
	///
	/// This is the only way bytes move between tiers in this harness, and it is
	/// deliberately the dumbest possible way: one `set` per pair, batched, no
	/// knowledge of what any key means. If passthrough were lossy anywhere — a key
	/// rewritten, a value truncated, an ordering assumed — this is where it shows.
	pub async fn replay_all(&self, pairs: &Keyspace) -> anyhow::Result<Keyspace> {
		let all: Vec<(&Vec<u8>, &Vec<u8>)> = pairs.iter().collect();
		for chunk in all.chunks(REPLAY_BATCH) {
			let txn = self.txn(TransactionType::Write).await?;
			for (key, val) in chunk {
				txn.set(Key::from(key.as_slice()), val.to_vec()).await?;
			}
			txn.commit().await?;
		}
		self.scan_all().await
	}

	/// The whole keyspace, in one pass.
	pub async fn scan_all(&self) -> anyhow::Result<Keyspace> {
		let txn = self.txn(TransactionType::Read).await?;
		let mut out = Keyspace::new();
		let mut resume: Option<Vec<u8>> = None;
		// Paged, resuming past the last key each time. A backend is allowed to
		// return fewer rows than the limit asked for, so the loop ends on an
		// *empty* page rather than on a short one — and asking for `u32::MAX` in a
		// single call is not an option either, because a real backend is allowed
		// to refuse it.
		loop {
			let from = resume.as_deref().unwrap_or(&[0x00]);
			let page = txn.scan(bytes(from, &[0xff]), PAGE, 0, None).await?;
			if page.values.is_empty() {
				break;
			}
			for (key, val) in page.values {
				// Past the key, not at it: the range's start is inclusive, so
				// resuming from the key itself would hand back the same page for
				// ever.
				resume = Some(Key::from(key.as_slice()).next().into_vec());
				out.insert(key, val);
			}
		}
		// Cancelled rather than left to fall out of scope: an open transaction
		// pins our tier's GC horizon for the life of the process, and the next
		// store this test opens would then see collection switched off. That is the
		// leak PROGRESS.md names, and this is where it would be introduced.
		txn.cancel().await?;
		Ok(out)
	}

	/// Every `(key, value)` in one range, in one transaction's worth of reading.
	pub async fn scan(&self, range: &KeyRange<'static>) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
		let txn = self.txn(TransactionType::Read).await?;
		let out = txn.scan(range.clone(), u32::MAX, 0, None).await?.values;
		txn.cancel().await?;
		Ok(out)
	}

	pub async fn shutdown(self) -> anyhow::Result<()> {
		Ok(self.builder.shutdown().await?)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Authoring the dataset with the real front end
// ─────────────────────────────────────────────────────────────────────────────

/// Run [`WORKLOAD`] against a real `Datastore` on upstream's `rocksdb:` and
/// return the keyspace it produced.
///
/// In-process, through the composer seam (R-0036) rather than through
/// `surrealdb_server::init`: `init` owns `std::env::args()`, installs the global
/// tracing subscriber and does an online version check, none of which a test can
/// do twice. `without_maintenance_tasks` because the harness compares what a
/// query *committed*, not what a background task later did to it — an index
/// compaction landing between two runs would be a race, not a finding.
pub async fn author_with_surrealql(path: &str) -> anyhow::Result<Keyspace> {
	let ds: Arc<Datastore> =
		Builder::new().without_maintenance_tasks().build_with_factory_path(path, DsComposer::new()).await?;
	ds.bootstrap().await?;

	let session = Session::owner().with_rt(true);
	for (i, result) in ds.execute(WORKLOAD, &session, None).await?.iter().enumerate() {
		// Every statement must succeed. A workload half-applied would author a
		// dataset nobody intended, and the round trip would then "pass" on it.
		if let Err(err) = &result.result {
			anyhow::bail!("workload statement {i} failed: {err}");
		}
	}

	// Paged for the same reason `Backend::scan_all` pages: one oversized scan is
	// something a backend is allowed to refuse.
	let txn = ds.transaction(TransactionType::Read).await?;
	let mut out = Keyspace::new();
	let mut resume: Option<Vec<u8>> = None;
	loop {
		let from = resume.as_deref().unwrap_or(&[0x00]);
		let page = txn.scan_raw(RawRange::of_bytes(from, &[0xff]), PAGE, 0, None).await?;
		if page.is_empty() {
			break;
		}
		for (key, val) in page {
			resume = Some(Key::from(key.as_slice()).next().into_vec());
			out.insert(key, val);
		}
	}
	txn.cancel().await?;
	ds.shutdown().await?;
	Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Scratch space
// ─────────────────────────────────────────────────────────────────────────────

/// A scratch directory that removes itself.
///
/// `Drop` rather than an explicit call at the end of each test, so a failing
/// assertion does not leak a RocksDB directory per run. A process that panics
/// outright still leaks one; the name carries the pid so that is visible rather
/// than silent.
pub struct Scratch {
	path: PathBuf,
}

impl Scratch {
	pub fn new(label: &str) -> anyhow::Result<Self> {
		let path = std::env::temp_dir().join(format!("open-surrealdb-ds-golden-{}-{label}", std::process::id()));
		let _ = std::fs::remove_dir_all(&path);
		std::fs::create_dir_all(&path)?;
		Ok(Self { path })
	}

	/// An upstream `rocksdb:` datastore path inside this scratch directory.
	pub fn backend(&self, name: &str) -> String {
		format!("rocksdb:{}/{}", self.path.display(), name)
	}

	/// The same directory as a plain filesystem path, which is what a non-SurrealDB
	/// reader needs — the datastore path above carries a `rocksdb:` scheme prefix
	/// that only the registry knows how to strip.
	pub fn dir(&self, name: &str) -> PathBuf {
		self.path.join(name)
	}

	pub fn path(&self) -> &Path {
		&self.path
	}
}

impl Drop for Scratch {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.path);
	}
}