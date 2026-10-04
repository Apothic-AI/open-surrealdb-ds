//! ADR-0008, falsification tranche: can we open a database upstream wrote?
//!
//! # The claim under test
//!
//! ADR-0008 says our local durable tier must write a database a stock `surreal`
//! can open, and open one upstream wrote — full on-disk interchangeability, not
//! merely KV-boundary fidelity. That is thousands of lines of work, and ADR-0008
//! itself says to de-risk it the way ADR-0003 de-risked the extension seam: find
//! out cheaply whether it is possible before committing to it.
//!
//! So this is the smallest experiment that can kill the decision. It builds no
//! tier, and it is deliberately not a step toward one:
//!
//! ```text
//!   upstream writes          upstream reads it back        we read it back
//!   ──────────────           ────────────────────          ───────────────
//!   SurrealQL ──▶ rocksdb:/tmp/…  rocksdb:<same dir>        rocksdb (ours)
//!                OPTIONS, MANIFEST,  via the KV contract     prefix extractor
//!                WAL, SST          ──▶ 55 keys, 1526 bytes ──▶ compared, byte
//!                                                            for byte
//! ```
//!
//! The middle column matters and is easy to get wrong. The obvious design —
//! dump the keyspace through a live `Datastore`, shut down, then read the closed
//! directory — **does not work**, and
//! [`a_clean_shutdown_rewrites_the_node_row`] is the test that says why in one
//! measured byte. Both sides of the comparison have to read the same quiesced
//! directory, so the writer's own reader is what establishes what is in it.
//!
//! # Why the prefix extractor looked like the crux, and what it actually is
//!
//! `docs/evidence/upstream-rocksdb-OPTIONS-3.3.0.txt`, which our own binary
//! produced, records `prefix_extractor=surrealdb.TablePrefix.v1`. The tranche
//! brief treated that name as a hard requirement, on the premise that RocksDB
//! resolves it *by name* when opening a database and that an unregistered name is
//! an open failure rather than a warning.
//!
//! **That premise is wrong**, and
//! [`the_named_extractor_is_not_what_makes_the_open_succeed`] is the test that
//! shows it: the directory opens and reads back identically with *no* extractor
//! registered, and with one registered under a name upstream never wrote.
//! `DB::open` does not read the on-disk `OPTIONS` file at all — the options in
//! force are the ones the caller passed in. The recorded name is consulted only
//! by an explicit `Options::load_from_file`, which nothing here performs.
//!
//! That does not make the extractor optional. It makes it a **correctness**
//! obligation rather than an **openability** one — and a nastier one than the
//! brief assumed, because a wrong extractor does not fail: under
//! `ReadOptions::set_prefix_same_as_start` it returns a **wider** range than
//! asked for, silently. [`our_extractor_matches_the_documented_layout`] checks
//! that ours computes the documented prefix on the keys upstream actually wrote,
//! and [`a_mismatched_extractor_silently_widens_a_prefix_restricted_read`] shows
//! what happens when one does not.
//!
//! # What this does NOT prove
//!
//! Stated here so a green run is not read as more than it is:
//!
//! 1. **Nothing about writing.** We read a directory; we never write one that
//!    upstream opens. That is the other half of ADR-0008 and it is untested.
//! 2. **Only the key classes this dataset reaches.** No class is proven
//!    complete — the same limit the golden harness records.
//! 3. **A small dataset.** 55 keys never leave one level, so compression, blob
//!    files, two-level indexes and partitioned filters are *configured* in the
//!    `OPTIONS` file and *exercised* nowhere.
//!    [`upstream_leaves_sst_files_behind`] is the one thing checked about that.
//! 4. **The `OPTIONS` file records settings, not behaviour.** It names codecs
//!    that are not compiled into the binding upstream links — `kZSTD`, for one —
//!    and nothing has ever failed, because the compaction that would have used
//!    them never ran.
//!
//! # Licence
//!
//! BUSL-1.1, like the crate it lives in: it links `surrealdb-server`, and the
//! reader is SurrealDB's own fork of `rocksdb`. See ../NOTICE and DECISIONS.md
//! ADR-0002.

mod common;

use std::path::Path;

use common::{Backend, Keyspace, Scratch, author_with_surrealql, hex, show};
use rocksdb::{DB, Direction, IteratorMode, Options, ReadOptions, SliceTransform};

// ─────────────────────────────────────────────────────────────────────────────
// `surrealdb.TablePrefix.v1`
// ─────────────────────────────────────────────────────────────────────────────

/// The name upstream persists into the `OPTIONS` file, and therefore the name to
/// register under for any future `Options::load_from_file` round trip to work.
/// See `PROVENANCE.md` R-0050.
const NAME: &str = "surrealdb.TablePrefix.v1";

/// Byte offset at which the null-terminated table name starts in a well-formed
/// table-level key: `/`, `*`, ns:4, `*`, db:4, `*` — 12 bytes.
const TB_START: usize = 12;

/// The shortest possible in-domain key: the 12-byte header, a zero-length table
/// name encoded as the bare `\0` terminator, and one discriminator byte.
const MIN_LEN: usize = TB_START + 2;

/// The exclusive end of a key's table+category prefix, or `None` if the key is
/// not in the table-level domain.
///
/// Written from the documented layout, not copied. A table-level key is `/`, `*`,
/// a 4-byte namespace id, `*`, a 4-byte database id, `*`, a null-terminated
/// table name, and then a one-byte discriminator saying what follows: `*`
/// records, `+` index entries, `!` table metadata, `~` graph edges, `&` refs.
/// The prefix is everything up to *and including* that discriminator.
///
/// Everything else — root, namespace and database metadata, change feeds,
/// id-sequence state, node rows — is out of domain, because position 1, 6 or 11
/// is not the `*` the header requires, or because there is no discriminator to
/// include. The namespace and database ids may legitimately contain zero bytes,
/// so the scan for the terminator starts at [`TB_START`] and nowhere earlier.
fn prefix_end(key: &[u8]) -> Option<usize> {
	if key.len() < MIN_LEN {
		return None;
	}
	if key[0] != b'/' || key[1] != b'*' || key[6] != b'*' || key[11] != b'*' {
		return None;
	}
	let terminator = TB_START + key.get(TB_START..)?.iter().position(|&byte| byte == 0)?;
	// No discriminator byte means the key is ambiguous — a bare table root, say —
	// so it cannot be classified into a category and is out of domain too.
	if terminator + 1 >= key.len() {
		return None;
	}
	Some(terminator + 2)
}

/// RocksDB requires `Transform` to return a sub-slice for *every* key, including
/// out-of-domain ones, so an unrecognised key is returned whole and kept out of
/// the bloom filter by `in_domain` instead.
fn transform(key: &[u8]) -> &[u8] {
	prefix_end(key).map_or(key, |end| key.get(..end).unwrap_or(key))
}

/// Whether this key participates in the prefix bloom filter at all.
fn in_domain(key: &[u8]) -> bool {
	prefix_end(key).is_some()
}

/// Our `SliceTransform`, registered under upstream's exact name.
fn table_prefix_v1() -> SliceTransform {
	SliceTransform::create(NAME, transform, Some(in_domain))
}

// ─────────────────────────────────────────────────────────────────────────────
// Reading a directory two ways
// ─────────────────────────────────────────────────────────────────────────────

/// Open a directory with **our** RocksDB binding and read the whole keyspace.
///
/// `create_if_missing` stays at its default of off, so a path that is not a
/// database is an error rather than a silently created empty one — a reader that
/// cannot fail on a wrong path proves nothing about a right one.
///
/// `verify_checksums` is on: the SSTs carry `checksum=kXXH3`, and forcing the
/// check means a read served out of an SST file is verified against it rather
/// than trusted.
fn read_with_our_reader(dir: &Path, extractor: Option<SliceTransform>) -> anyhow::Result<Keyspace> {
	let mut opts = Options::default();
	if let Some(extractor) = extractor {
		opts.set_prefix_extractor(extractor);
	}
	let db = DB::open(&opts, dir)?;

	let mut read_opts = ReadOptions::default();
	read_opts.set_verify_checksums(true);

	let mut out = Keyspace::new();
	for item in db.iterator_opt(IteratorMode::Start, read_opts) {
		let (key, val) = item?;
		out.insert(key.to_vec(), val.to_vec());
	}
	Ok(out)
}

/// Read the same directory through **upstream's** reader, via the KV contract.
///
/// This is what establishes what is actually in the directory. It has to be a
/// separate open, not a dump taken while the writer was running: see
/// [`a_clean_shutdown_rewrites_the_node_row`].
async fn read_with_upstreams_reader(dir: &Path) -> anyhow::Result<Keyspace> {
	let backend = Backend::open(&format!("rocksdb:{}", dir.display())).await?;
	let keyspace = backend.scan_all().await?;
	backend.shutdown().await?;
	Ok(keyspace)
}

/// Author the dataset with upstream's engine and let it close, so the directory
/// is quiesced before anything reads it.
async fn author_and_close(scratch: &Scratch, label: &str) -> anyhow::Result<(std::path::PathBuf, Keyspace)> {
	let dir = scratch.dir(label);
	let source = author_with_surrealql(&format!("rocksdb:{}", dir.display())).await?;
	Ok((dir, source))
}

/// Count files in a directory with a given extension.
fn count_ext(dir: &Path, extension: &str) -> usize {
	count_where(dir, |name| name.extension().is_some_and(|ext| ext == extension))
}

/// Count files in a directory whose name starts with a prefix.
///
/// The `MANIFEST-000013` and `IDENTITY` spellings have no extension to match on,
/// which is why this is separate from [`count_ext`].
fn count_where(dir: &Path, matches: impl Fn(&std::path::Path) -> bool) -> usize {
	std::fs::read_dir(dir)
		.map(|entries| entries.filter_map(Result::ok).filter(|entry| matches(&entry.path())).count())
		.unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// The checks
// ─────────────────────────────────────────────────────────────────────────────

/// The headline: a directory upstream wrote, opened by our own reader, byte for
/// byte against upstream's own reader on the same directory.
///
/// This is the falsification test for ADR-0008. If it fails, the decision is
/// wrong at its foundation and no tier should be written.
#[tokio::test(flavor = "multi_thread")]
async fn our_reader_opens_an_upstream_written_directory() -> anyhow::Result<()> {
	let scratch = Scratch::new("interop")?;
	let (dir, before_shutdown) = author_and_close(&scratch, "upstream").await?;

	// Both sides read the closed directory, one with upstream's binding and one
	// with ours. Sequential, not concurrent: RocksDB holds an exclusive lock, and
	// two readers of one directory is not a thing it offers.
	let theirs = read_with_upstreams_reader(&dir).await?;
	let ours = read_with_our_reader(&dir, Some(table_prefix_v1()))?;
	compare("upstream-written directory read by our own binding", &theirs, &ours)?;

	// The dataset must still be the dataset, or the comparison is over nothing:
	// reading a directory we quietly emptied would also produce zero differences.
	assert_eq!(theirs.len(), before_shutdown.len(), "the closed directory holds a different number of keys than the running one did");

	let keys_compared = theirs.len();
	let bytes_compared = theirs.values().map(Vec::len).sum::<usize>();
	println!(
		"interop: {keys_compared} keys / {bytes_compared} value bytes in an upstream-written directory,\n  \
		 read by our own rocksdb binding and by upstream's, every key and every value equal\n  \
		 files upstream left: {} SST, {} log, {} MANIFEST",
		count_ext(&dir, "sst"),
		count_ext(&dir, "log"),
		count_where(&dir, |name| name.file_name().is_some_and(|f| f.to_string_lossy().starts_with("MANIFEST"))),
	);
	Ok(())
}

/// Upstream leaves SST files behind, so the read above really did come out of the
/// block-based table format — not just a replayed write-ahead log.
///
/// Its own test rather than an assertion inside the headline one, so "we read
/// RocksDB files" is a claim with a name and a failure of its own. If this ever
/// drops to zero, the headline test has silently become a WAL reader and its
/// green means much less than it says.
#[tokio::test(flavor = "multi_thread")]
async fn upstream_leaves_sst_files_behind() -> anyhow::Result<()> {
	let scratch = Scratch::new("sst")?;
	let (dir, _) = author_and_close(&scratch, "upstream").await?;

	let ssts = count_ext(&dir, "sst");
	assert!(ssts > 0, "upstream left no .sst files in {}; the interop read would only have replayed the WAL", dir.display());
	println!("sst: upstream left {ssts} .sst file(s) in {}", dir.display());
	Ok(())
}

/// The trap that makes the obvious version of this harness wrong.
///
/// The first attempt at the headline test dumped the keyspace through a live
/// `Datastore`, shut down, and then read the closed directory. It failed on
/// **exactly one byte of one key**, reproducibly: a `/!nd{nd}` node row whose
/// byte 27 read `00` before shutdown and `01` after.
///
/// That byte is `Node.gc` — the archived flag — and a clean shutdown sets it,
/// because upstream archives its own node on the way out. A harness that dumps
/// through a live writer and compares against the directory therefore compares
/// two different moments, and would report a byte-compatibility failure for a
/// shutdown side effect.
///
/// The point of pinning it here is that it is easy to reintroduce: any future
/// interop or tier test that reads a directory needs to know the write has to be
/// quiesced first.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_shutdown_rewrites_the_node_row() -> anyhow::Result<()> {
	let scratch = Scratch::new("shutdown")?;
	let (dir, before_shutdown) = author_and_close(&scratch, "upstream").await?;
	let after_shutdown = read_with_our_reader(&dir, Some(table_prefix_v1()))?;

	let changed: Vec<(&Vec<u8>, &Vec<u8>, &Vec<u8>)> = before_shutdown
		.iter()
		.filter_map(|(key, before)| after_shutdown.get(key).filter(|after| *after != before).map(|after| (key, before, after)))
		.collect();

	// The claim is narrow and measured: the only thing a clean shutdown rewrites
	// is the node row, by a single byte, and only the ones this dataset has.
	let node_rows = changed.iter().filter(|(key, ..)| key.starts_with(b"/!nd")).count();
	assert!(
		changed.iter().all(|(key, ..)| key.starts_with(b"/!nd")),
		"a clean shutdown changed something other than a node row: {:?}",
		changed.iter().filter(|(key, ..)| !key.starts_with(b"/!nd")).map(|(key, ..)| show(key)).collect::<Vec<_>>()
	);
	assert!(node_rows > 0, "expected at least one node row to be rewritten on shutdown, none was");

	for (key, before, after) in &changed {
		assert_eq!(before.len(), after.len(), "the node row changed length, not a flag byte: {}", show(key));
		assert_eq!(before.len(), 29, "unexpected node row length {} for {}", before.len(), show(key));
		let at = before.iter().zip(after.iter()).position(|(a, b)| a != b).expect("filtered on a difference");
		// Byte 27 of 29: a 2-byte revision header, a 16-byte node id, and an
		// 8-byte heartbeat timestamp come first, so this is the byte after the
		// timestamp — the `gc` flag, then the `http_endpoint` discriminant.
		assert_eq!(at, 27, "the node row changed at byte {at}, not at the `gc` flag (27): {}", show(key));
		assert_eq!((before[at], after[at]), (0, 1), "the node row's `gc` flag did not go false -> true");
		println!("shutdown: node row {} — byte 27 (Node.gc) went 00 -> 01 across a clean shutdown", show(key));
	}

	println!("shutdown: {} of {} keys rewritten by a clean shutdown, all of them node rows", changed.len(), before_shutdown.len());
	Ok(())
}

/// The brief's premise, checked rather than assumed.
///
/// It said a `prefix_extractor` not registered under exactly
/// `surrealdb.TablePrefix.v1` is **an open failure**. It is not: the directory
/// opens, and reads back identically, with no extractor registered at all.
///
/// The reason is that `DB::open` never reads the on-disk `OPTIONS` file. The
/// options in force are the ones passed in code, and the `prefix_extractor` line
/// in that file is consulted only by an explicit `Options::load_from_file`, which
/// nothing in this path performs. So the recorded name is a correctness
/// obligation for reading SST bloom filters, not an openability one.
///
/// Here because a false premise in a plan is worth as much as a true one, and
/// because if a future RocksDB *does* start resolving the name on open, this is
/// the test that will notice.
#[tokio::test(flavor = "multi_thread")]
async fn the_named_extractor_is_not_what_makes_the_open_succeed() -> anyhow::Result<()> {
	let scratch = Scratch::new("noextract")?;
	let (dir, _) = author_and_close(&scratch, "upstream").await?;
	let theirs = read_with_upstreams_reader(&dir).await?;

	// No extractor at all, so not even a byte-identical one.
	let read = read_with_our_reader(&dir, None)?;
	compare("upstream-written directory with no prefix extractor registered", &theirs, &read)?;

	// And one registered under a name upstream never wrote, which is the shape of
	// the failure the brief predicted.
	let mut opts = Options::default();
	opts.set_prefix_extractor(SliceTransform::create("wrong.Name.v0", transform, Some(in_domain)));
	let count = DB::open(&opts, &dir)?.iterator(IteratorMode::Start).count();

	println!(
		"extractor: the same directory opens and reads all {} keys with no prefix extractor, and with one\n  \
		 registered as `wrong.Name.v0`; DB::open does not read the on-disk OPTIONS file, so the recorded\n  \
		 name `surrealdb.TablePrefix.v1` is not what admits the open",
		theirs.len()
	);
	assert_eq!(count, theirs.len(), "the mis-named extractor read a different number of keys");
	Ok(())
}

/// Our extractor agrees with the documented layout, on the real dataset.
///
/// The name is not enough. Bloom filters are keyed by the *prefix*, so an
/// extractor registered under the right name and computing the wrong prefix reads
/// SST filters built for a different keying. This checks the computation against
/// the layout on the keys upstream actually wrote, and reports how many of them
/// are in the domain at all — a name nothing depends on would be worth nothing.
#[tokio::test(flavor = "multi_thread")]
async fn our_extractor_matches_the_documented_layout() -> anyhow::Result<()> {
	let scratch = Scratch::new("extractor")?;
	let (dir, _) = author_and_close(&scratch, "upstream").await?;
	let keyspace = read_with_our_reader(&dir, Some(table_prefix_v1()))?;

	let domain: Vec<&Vec<u8>> = keyspace.keys().filter(|key| in_domain(key)).collect();
	let out_of_domain: Vec<&Vec<u8>> = keyspace.keys().filter(|key| !in_domain(key)).collect();

	assert!(!domain.is_empty(), "no key in the dataset is in the extractor's domain, so the extractor is untested");
	for key in &domain {
		let prefix = transform(key);
		// The prefix is everything up to and including the discriminator, so it
		// always begins with the 12-byte header and never runs past the key.
		assert!(prefix.len() >= MIN_LEN, "prefix shorter than the shortest in-domain key: {}", show(prefix));
		assert!(prefix.len() <= key.len());
		assert_eq!(prefix.first(), Some(&b'/'), "prefix does not start with the header: {}", show(prefix));
		// The discriminator is the last byte of the prefix, and it is what makes
		// records, indexes, metadata and graph edges distinguishable.
		assert!(
			matches!(prefix.last(), Some(b'*' | b'+' | b'!' | b'~' | b'&')),
			"prefix does not end in a discriminator: {}",
			show(prefix)
		);
		assert_eq!(prefix, &key[..prefix.len()], "the prefix is not a prefix of its own key");
	}
	// Out of domain means *returned unchanged*, which is the RocksDB contract for
	// Transform and the only reason an unrecognised key cannot corrupt a filter.
	for key in &out_of_domain {
		assert_eq!(transform(key), key.as_slice(), "out-of-domain key was transformed");
	}

	// Two keys in one table's record namespace share a prefix; the extractor
	// exists precisely so that a scan of one table can skip another table's SSTs,
	// so check that rather than assume it.
	let prefix_of = |key: &[u8]| key.get(..prefix_end(key).expect("in domain")).expect("in domain").to_vec();
	let records: Vec<Vec<u8>> = domain
		.iter()
		.map(|key| key.as_slice())
		.filter(|key| key.ends_with(b"\x00") && prefix_end(key).is_some_and(|end| key[end - 1] == b'*'))
		.map(<[u8]>::to_vec)
		.collect();
	assert!(records.len() >= 2, "expected at least two record keys, found {}", records.len());
	assert_eq!(prefix_of(&records[0]), prefix_of(&records[1]), "two records in one table produced different prefixes");
	// …and the prefix carries the table name, so two *different* tables must not
	// collide into one prefix. `person` and the `knows` edge table both appear.
	let distinct: std::collections::BTreeSet<Vec<u8>> = domain.iter().map(|key| prefix_of(key)).collect();
	assert!(distinct.len() > 1, "every key in the dataset produced the same prefix, so the extractor distinguishes nothing");

	println!(
		"extractor: {}/{} keys in domain, {} out of domain (root, namespace/database metadata,\n  \
		 id-sequence state, node rows); every in-domain prefix ends in a discriminator",
		domain.len(),
		keyspace.len(),
		out_of_domain.len()
	);
	Ok(())
}

/// Why the extractor still has to be right, now demonstrated rather than asserted.
///
/// [`the_named_extractor_is_not_what_makes_the_open_succeed`] shows the *name* is
/// not what admits the open. This shows the *computation* is load-bearing, and
/// that getting it wrong fails silently.
///
/// The lever is `ReadOptions::set_prefix_same_as_start`, which restricts an
/// iterator to keys sharing the seek key's extracted prefix. Our extractor
/// computes `/*<ns>*<db>*person\0*` for a seek into the `person` record namespace,
/// so the restriction holds and the iterator stops when the discriminator
/// changes — 2 rows, the two record keys.
///
/// A deliberately wrong extractor — every key truncated to its first byte, always
/// in domain — collapses every key to the same one-byte prefix, so the restriction
/// stops narrowing anything and the iterator runs on past the index and graph-edge
/// keys: 12 rows, **no error**. Neither RocksDB nor the reader notices.
///
/// That is the shape of the hazard ADR-0008 has to live with: a wrong extractor
/// does not fail the open, does not fail the read, and does not fail loudly. It
/// returns a wider answer than the caller asked for.
#[tokio::test(flavor = "multi_thread")]
async fn a_mismatched_extractor_silently_widens_a_prefix_restricted_read() -> anyhow::Result<()> {
	let scratch = Scratch::new("widen")?;
	let (dir, _) = author_and_close(&scratch, "upstream").await?;

	// A well-formed table-level seek start: `/`, `*`, ns:4, `*`, db:4, `*`,
	// `person`, `\0`, then the record discriminator and a low id byte.
	let from: &[u8] = b"/*\x00\x00\x00\x00*\x00\x00\x00\x00*person\x00*\x00";
	assert!(in_domain(from), "the probe's own seek key is out of the extractor's domain, so it would prove nothing");

	let narrowed = prefix_restricted_seek(&dir, Some(table_prefix_v1()), from);
	let widened = prefix_restricted_seek(
		&dir,
		Some(SliceTransform::create("wrong.Name.v0", |key| key.get(..1).unwrap_or(key), Some(|_| true))),
		from,
	);
	let no_extractor = prefix_restricted_seek(&dir, None, from);

	// Without the flag the same seek returns the full run, which is the baseline
	// the two restricted reads are being compared against.
	let unrestricted = prefix_restricted_seek_flag(&dir, Some(table_prefix_v1()), from, false);
	assert_eq!(narrowed, 2, "expected the correct extractor to narrow the seek to the record keys");
	assert_eq!(widened, unrestricted, "a mismatched extractor silently widened the read instead of narrowing it");
	assert_eq!(no_extractor, unrestricted, "no extractor at all is the same silent widening");

	println!(
		"extractor: the same prefix-restricted seek returns {narrowed} rows with our extractor, {widened} with a\n  \
		 mismatched one and {no_extractor} with none — wider, not an error, which is the whole hazard",
	);
	Ok(())
}

/// Rows returned by a forward seek from `from` with `prefix_same_as_start` set.
///
/// Sequential because RocksDB holds an exclusive lock: two readers of one
/// directory is not a thing it offers.
fn prefix_restricted_seek(dir: &Path, extractor: Option<SliceTransform>, from: &[u8]) -> usize {
	prefix_restricted_seek_flag(dir, extractor, from, true)
}

/// [`prefix_restricted_seek`] with the flag under the tester's control.
fn prefix_restricted_seek_flag(dir: &Path, extractor: Option<SliceTransform>, from: &[u8], same_as_start: bool) -> usize {
	let mut opts = Options::default();
	if let Some(extractor) = extractor {
		opts.set_prefix_extractor(extractor);
	}
	let db = DB::open(&opts, dir).expect("the directory opens; this is about what the read returns, not whether it does");

	let mut read_opts = ReadOptions::default();
	read_opts.set_prefix_same_as_start(same_as_start);
	let count = db.iterator_opt(IteratorMode::From(from, Direction::Forward), read_opts).count();
	drop(db);
	count
}

/// The comparison can fail, and so can the reader it is checking.
///
/// A comparison that cannot reject anything would pass against any directory at
/// all, including an empty one. So: flip one bit in a value we hand back, require
/// the comparison to notice, and require it to name the byte it noticed.
///
/// Same discipline the golden harness applied to `DsTxn::set`, applied here to
/// the read path instead of the write path.
#[tokio::test(flavor = "multi_thread")]
async fn the_comparison_catches_a_flipped_bit() -> anyhow::Result<()> {
	let scratch = Scratch::new("falsify")?;
	let (dir, _) = author_and_close(&scratch, "upstream").await?;
	let theirs = read_with_upstreams_reader(&dir).await?;
	let read = read_with_our_reader(&dir, Some(table_prefix_v1()))?;
	compare("unmodified read, which must pass", &theirs, &read)?;

	// The low bit of the last byte of the largest value in the dataset, so the
	// damage lands in the middle of a real record rather than in a one-byte
	// tombstone.
	let (victim_key, victim_val) =
		theirs.iter().max_by_key(|(_, val)| val.len()).ok_or_else(|| anyhow::anyhow!("dataset has no values"))?;
	let at = victim_val.len() - 1;
	let expected_byte = victim_val[at];

	let mut corrupted = read.clone();
	let got = corrupted.get_mut(victim_key).expect("key was just read");
	got[at] ^= 0b0000_0001;

	let err = compare("deliberately corrupted read", &theirs, &corrupted)
		.expect_err("the comparison accepted a value with one bit flipped");
	let message = err.to_string();
	assert!(message.contains(&format!("first differs at byte {at}")), "the comparison did not name the byte it caught:\n{message}");
	assert!(
		message.contains(&hex(&[expected_byte])) && message.contains(&hex(&[expected_byte ^ 0b0000_0001])),
		"the comparison did not name both bytes:\n{message}"
	);
	println!(
		"falsify: one flipped bit caught at byte {at} of {}, the last byte of the largest value in the dataset\n{}",
		victim_val.len(),
		message
	);
	Ok(())
}

/// Compare two keyspaces exactly, naming the first difference.
///
/// The same contract as the golden harness's comparison and for the same reason:
/// exact, with nothing excused, because both sides hold the same bytes — one side
/// wrote them and the other read them back.
fn compare(what: &str, expected: &Keyspace, actual: &Keyspace) -> anyhow::Result<()> {
	let missing: Vec<&Vec<u8>> = expected.keys().filter(|key| !actual.contains_key(*key)).collect();
	let extra: Vec<&Vec<u8>> = actual.keys().filter(|key| !expected.contains_key(*key)).collect();
	let changed: Vec<&Vec<u8>> =
		expected.keys().filter(|key| actual.get(*key).is_some_and(|got| *got != expected[*key])).collect();

	if missing.is_empty() && extra.is_empty() && changed.is_empty() {
		return Ok(());
	}

	let mut report = format!("{what}: the two keyspaces differ\n");
	report.push_str(&format!("  {} keys expected, {} found\n", expected.len(), actual.len()));
	for key in missing.iter().take(4) {
		report.push_str(&format!("  missing:  {}\n", show(key)));
	}
	for key in extra.iter().take(4) {
		report.push_str(&format!("  extra:    {}\n", show(key)));
	}
	for key in changed.iter().take(4) {
		let want = &expected[*key];
		let got = &actual[*key];
		// Name the offset: "this key differs" leaves the reader to bisect a
		// 123-byte value by hand.
		let at = want.iter().zip(got.iter()).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
		report.push_str(&format!(
			"  changed:  {} — first differs at byte {at}: expected {}, found {}\n",
			show(key),
			want.get(at..at + 1).map(hex).unwrap_or_else(|| "<end of value>".to_owned()),
			got.get(at..at + 1).map(hex).unwrap_or_else(|| "<end of value>".to_owned())
		));
	}
	anyhow::bail!(report)
}