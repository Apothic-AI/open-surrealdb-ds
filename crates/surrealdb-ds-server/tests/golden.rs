//! The golden-file round trip: our storage tier against upstream's, byte for byte.
//!
//! # What this proves, and what it does not
//!
//! L2 — "byte compatibility" — is this project's definition of 1:1: our node and
//! a real SurrealDB node read and write the same data. `Transactable` is a
//! **byte-level** contract (`set(key: Key, val: Val)`, `get(key) -> Option<Val>`),
//! so the whole question collapses to one property: *does our tier hand back the
//! bytes it was given?* Not a re-implementation of the encoder, not a decode
//! step, not a normalisation. Passthrough.
//!
//! So this harness is built to make that one property falsifiable, and it does it
//! in two independent ways.
//!
//! ## Leg A — the round trip, absolute, no exclusions
//!
//! ```text
//!   upstream authors           our tier holds            upstream re-reads
//!   ───────────────            ─────────────             ────────────────
//!   SurrealQL ──▶ rocksdb:      replay src ──▶ ds+mem://   replay our dump
//!                 dump = src               dump = ours      ──▶ rocksdb:
//!                                                 │              dump = back
//!                                                 └─ src == ours == back
//! ```
//!
//! Every byte is compared. There is no allow-list, no normalisation, and no
//! "known-divergent" bucket, because the bytes under comparison are bytes that
//! were **copied**, not bytes that a run happened to **generate**. Upstream writes
//! the dataset once; we replay it in and read it out; upstream replays our
//! output in and reads it back. This is the literal wording of Phase 1's exit
//! criterion, and the third store is not redundant — it is the control that makes
//! a leg-A failure attributable to our tier rather than to the replay mechanism.
//!
//! ## Leg B — the committed golden manifest
//!
//! A committed artifact is worth more than a passing run, because it turns
//! upstream's layout into something a reviewer can diff. But a fresh authoring
//! run is **not** byte-reproducible, and pretending otherwise would be the easiest
//! way to make this harness lie. Measured, not assumed: the same SurrealQL
//! workload run twice against upstream's own `rocksdb:` reproduces only about
//! two thirds of its keys. The rest carry a fresh random identifier on every run
//! — node rows, namespace-id state, table doc-id and index-id state, index-build
//! state, and the table definition itself, whose `cache_*_ts` fields are five
//! `Uuid::new_v7()` watermarks.
//!
//! So the manifest is a list of the keys upstream *does* reproduce, byte for
//! byte, plus the number of the ones it does not:
//!
//! ```text
//! K <key-hex> <val-hex>     # `-` for an empty value; plenty of upstream keys store one
//! excluded <count>
//! ```
//!
//! Which keys land on which side is **measured on every run** by authoring upstream
//! twice and diffing, never asserted from a hand-kept list — so a key that stops
//! being reproducible, or a dataset that grows one, fails the check instead of
//! quietly narrowing it.
//!
//! All-or-nothing per key, and that is a considered choice rather than the lazy
//! one. Masking only the bytes that differ was tried first and is a trap:
//! `Uuid::new_v7()` embeds a millisecond timestamp, so two runs milliseconds apart
//! share its high bytes and differ only in the random tail. Pinning the shared
//! bytes looks like extra coverage and is really a committed file that goes stale
//! within minutes. A *whole* value two runs agree on cannot contain a UUID at all,
//! which is what makes this the only time-stable rule available without decoding
//! the value.
//!
//! What the rule costs, stated plainly: the table definitions — which is where
//! `PERMISSIONS` lives — carry those five watermarks, so they are among the
//! excluded and their bytes are **not** pinned in the committed file. They are
//! compared byte for byte by leg A, which has no exclusions at all. See
//! `what_this_does_not_prove` in the report this harness was built for; the
//! limitation is real and it is not papered over here.
//!
//! # The dataset
//!
//! Written as SurrealQL and executed by the real front end against a real
//! `Datastore`, so the keys are whatever upstream's `keyspace!` actually emits —
//! not a hand-picked subset. See [`WORKLOAD`] for the coverage and what each part
//! is there to exercise.
//!
//! # The dataset
//!
//! Written as SurrealQL and executed by the real front end against a real
//! `Datastore`, so the keys are whatever upstream's `keyspace!` actually emits —
//! not a hand-picked subset. See [`WORKLOAD`] for the coverage and what each part
//! is there to exercise.
//!
//! # Why not `surrealdb_server::init`
//!
//! The server path is process-global: `init` builds its own runtime and
//! `block_on`s the CLI over `std::env::args()`, and it installs the global
//! tracing subscriber. A test cannot call it twice, and it does an online version
//! check at startup. The seam it uses is one generic parameter, though
//! (`TransactionBuilderFactory`, R-0036), and `surrealdb_server` re-exports
//! `surrealdb_core` as `surrealdb_server::core` — so
//! `core::kvs::Builder::build_with_factory_path(path, composer)` builds a
//! `Datastore` against any registry, in-process, with no argv and no socket. That
//! is what this harness does. It costs no new dependency.
//!
//! # Licence
//!
//! BUSL-1.1, like the crate it lives in: it links `surrealdb-server`.
//! See ../NOTICE and DECISIONS.md ADR-0002.

mod common;

use std::fmt::Write as _;
use std::path::Path;

use common::{
	Backend, Keyspace, OURS, Scratch, VERSIONED_KEY, author_with_surrealql, bytes, hex, show, unhex,
};
use surrealdb_kvs::{Key, Transactable, TransactionType};

// ─────────────────────────────────────────────────────────────────────────────
// The manifest
// ─────────────────────────────────────────────────────────────────────────────

/// Where the committed manifest lives, relative to this file.
const GOLDEN_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/golden-3.3.0.txt");

/// Set to rewrite the manifest instead of checking against it.
///
/// Deliberately an environment variable rather than a second test: a test whose
/// job is to mutate a committed file is a test that can silently pass in CI
/// because somebody exported the wrong thing. The name says what it does.
const UPDATE_ENV: &str = "DS_GOLDEN_UPDATE";

/// One key in the manifest: bytes upstream produced identically on every run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
	key: Vec<u8>,
	val: Vec<u8>,
}

/// The manifest: every key upstream reproduces, plus the number it does not.
#[derive(Debug)]
struct Manifest {
	entries: Vec<Entry>,
	/// Keys upstream wrote whose **key bytes or value bytes** differ between two
	/// runs. They are outside the manifest entirely rather than recorded loosely:
	/// they carry a fresh `Uuid::new_v7()`, and no committed spelling of such a
	/// value would ever match again. The *count* is pinned, so a change in how
	/// many there are fails, and leg A compares their bytes absolutely.
	excluded: usize,
}

impl Manifest {
	fn contains(&self, key: &[u8]) -> bool {
		self.entries.iter().any(|entry| entry.key == key)
	}
}

/// The keys of `first` that `second` reproduced exactly, both key bytes and value.
///
/// Measured rather than declared, which is what keeps the manifest honest: a key
/// that stops being reproducible, or a dataset that grows one, changes what this
/// returns and the check fails instead of quietly narrowing.
///
/// **All or nothing, per key, and deliberately.** Masking just the bytes that
/// differ was tried and is a trap: `Uuid::new_v7()` embeds a millisecond
/// timestamp, so two runs milliseconds apart share its high bytes and differ only
/// in the random tail. Pinning the shared bytes looks like extra coverage and is
/// in fact a committed file that goes stale within minutes — the low timestamp
/// byte moves every millisecond, the next every 256, the next every 65 seconds.
/// A *whole* value that two runs agree on cannot contain a UUID at all (its
/// randomness would have to match), which is what makes this rule the only
/// time-stable one available without decoding the value.
fn reproducible(first: &Keyspace, second: &Keyspace) -> std::collections::BTreeSet<Vec<u8>> {
	first.keys().filter(|key| second.get(*key) == first.get(*key)).cloned().collect()
}

/// Serialise a keyspace as a manifest, given the keys upstream reproduced.
fn render(keyspace: &Keyspace, pinned: &std::collections::BTreeSet<Vec<u8>>) -> String {
	let mut out = String::new();
	let mut pinned_bytes = 0usize;
	for (key, val) in keyspace {
		if !pinned.contains(key) {
			continue;
		}
		pinned_bytes += val.len();
		// `-` for an empty value rather than nothing, so every line has the same
		// number of fields and the grammar stays trivial to parse by hand. Plenty
		// of upstream's keys store a zero-length value — graph edge keys, mostly.
		let rendered = if val.is_empty() { "-".to_owned() } else { hex(val) };
		let _ = writeln!(out, "K {} {rendered}", hex(key));
	}
	let excluded = keyspace.len() - pinned.len();
	let _ = writeln!(out, "excluded {excluded}");
	let _ = write!(
		out,
		"#\n\
		 # {} of {} keys pinned, carrying {pinned_bytes} of {} value bytes\n\
		 # The {excluded} unpinned keys are the ones upstream mints a fresh identifier into —\n\
		 # node rows, id-sequence state, index-build state, and the table definitions. They\n\
		 # are outside the comparison here and compared byte for byte by the round trip.\n\
		 # Generated by `make golden-update`, checked by `make golden`.\n",
		pinned.len(),
		keyspace.len(),
		keyspace.values().map(Vec::len).sum::<usize>()
	);
	out
}

/// Parse a manifest, refusing anything malformed.
///
/// A parse failure has to be an error rather than a skip: a silently skipped line
/// would turn a corrupt manifest into a green run.
fn parse(text: &str) -> anyhow::Result<Manifest> {
	let mut entries = Vec::new();
	let mut excluded = None;
	for (lineno, line) in text.lines().enumerate() {
		let line = line.trim();
		if line.is_empty() || line.starts_with('#') {
			continue;
		}
		let bad = |what: &str| anyhow::anyhow!("{}:{}: {what}", GOLDEN_PATH, lineno + 1);
		let fields: Vec<&str> = line.split(' ').collect();
		if fields[0] == "excluded" {
			if fields.len() != 2 {
				return Err(bad("expected `excluded <count>`"));
			}
			excluded = Some(fields[1].parse().map_err(|_| bad("bad excluded count"))?);
			continue;
		}
		match fields.as_slice() {
			["K", key, val] => entries.push(Entry {
				key: unhex(key).ok_or_else(|| bad("bad key hex"))?,
				val: match *val {
					"-" => Vec::new(),
					_ if val.len() % 2 != 0 => return Err(bad("value is not whole bytes")),
					_ => unhex(val).ok_or_else(|| bad("bad value hex"))?,
				},
			}),
			_ => return Err(bad("expected `K <key-hex> <val-hex>` or `excluded <count>`")),
		}
	}
	Ok(Manifest { entries, excluded: excluded.ok_or_else(|| anyhow::anyhow!("{GOLDEN_PATH}: no `excluded` line"))? })
}

/// Compare one live keyspace against a parsed manifest.
fn check(what: &str, expected: &Manifest, actual: &Keyspace) -> anyhow::Result<()> {
	let mut problems = Vec::new();
	for entry in &expected.entries {
		let Some(val) = actual.get(&entry.key) else {
			problems.push(format!("  missing:  {}", show(&entry.key)));
			continue;
		};
		if val == &entry.val {
			continue;
		}
		// Name the first differing offset rather than dumping two hex blobs: a diff
		// of hex tells you that two things differ and nothing about which.
		let first = entry.val.iter().zip(val).position(|(want, got)| want != got);
		let detail = match (first, entry.val.len() == val.len()) {
			(Some(at), true) => format!("first differs at byte {at}: expected {}, found {}", hex(&entry.val[at..at + 1]), hex(&val[at..at + 1])),
			_ => format!("{} value bytes expected, {} found", entry.val.len(), val.len()),
		};
		problems.push(format!("  {}: {detail}", show(&entry.key)));
	}

	// The other half of the keyspace: everything the manifest does not list has to
	// be a key upstream itself does not reproduce, and there has to be exactly as
	// many of them as the manifest recorded. Otherwise a key could quietly leave
	// the dataset and this check would never notice.
	let unlisted = actual.keys().filter(|key| !expected.contains(key)).count();
	if unlisted != expected.excluded {
		problems.push(format!(
			"  {unlisted} key(s) outside the manifest, but it records {} as excluded",
			expected.excluded
		));
	}

	if problems.is_empty() {
		return Ok(());
	}
	let mut report = format!("{what}: does not match {GOLDEN_PATH}\n");
	report.push_str(&problems.iter().take(8).cloned().collect::<Vec<_>>().join("\n"));
	if problems.len() > 8 {
		report.push_str(&format!("\n  … {} problem(s) in total", problems.len()));
	}
	anyhow::bail!(report)
}

// ─────────────────────────────────────────────────────────────────────────────
// Talking to a backend
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Authoring the dataset with the real front end
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Scratch space
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// The checks
// ─────────────────────────────────────────────────────────────────────────────

/// The golden manifest: upstream's keyspace, and then our engine's, against the
/// committed file.
///
/// Two authoring runs, because how much of the dataset upstream reproduces can
/// only be measured by diffing upstream against itself.
#[tokio::test(flavor = "multi_thread")]
async fn the_golden_manifest_still_describes_the_dataset() -> anyhow::Result<()> {
	let scratch = Scratch::new("manifest")?;
	let first = author_with_surrealql(&scratch.backend("author-1")).await?;
	let second = author_with_surrealql(&scratch.backend("author-2")).await?;
	let pinned = reproducible(&first, &second);
	let authored = render(&first, &pinned);

	// Ours is replayed rather than authored, because what is judged here is what
	// our tier *stores*; the byte-exact comparison of the whole keyspace, exclusions
	// and all, is the round trip's job.
	let ours = Backend::open(OURS).await?.replay_all(&first).await?;

	if std::env::var(UPDATE_ENV).is_ok() {
		// Refuse to write a manifest our own engine does not reproduce. Otherwise
		// `make golden-update` is a way to make the failing check pass, which is
		// exactly the wrong thing for a regenerate target to be.
		check("our engine vs the manifest about to be written", &parse(&authored)?, &ours)?;
		std::fs::write(Path::new(GOLDEN_PATH), &authored)?;
		println!("golden: wrote {GOLDEN_PATH}\n{}", summarise(&authored));
		return Ok(());
	}

	let committed = std::fs::read_to_string(GOLDEN_PATH).map_err(|err| {
		anyhow::anyhow!("{GOLDEN_PATH} is missing ({err}); regenerate it with `make golden-update`")
	})?;
	let expected = parse(&committed)?;

	check("upstream vs golden", &expected, &first)?;
	check("our engine vs golden", &expected, &ours)?;

	println!("golden: upstream and our engine both match {GOLDEN_PATH}\n{}", summarise(&committed));
	Ok(())
}

/// The manifest's own footer, so a run says what it covered.
fn summarise(manifest: &str) -> String {
	manifest
		.lines()
		.filter(|line| line.starts_with('#'))
		.map(|line| format!("  {line}"))
		.collect::<Vec<_>>()
		.join("\n")
}

/// The round trip: upstream → us → upstream, compared byte for byte.
///
/// The strong leg. Every key and every value, with nothing held back, because the
/// bytes under comparison were copied rather than regenerated.
#[tokio::test(flavor = "multi_thread")]
async fn the_dataset_round_trips_upstream_through_our_engine() -> anyhow::Result<()> {
	let scratch = Scratch::new("roundtrip")?;
	let source = author_with_surrealql(&scratch.backend("source")).await?;
	assert!(source.len() >= 50, "dataset is only {} keys; the workload lost coverage", source.len());

	let ours = Backend::open(OURS).await?.replay_all(&source).await?;
	compare_keyspaces("upstream -> our engine", &source, &ours)?;

	// The control. Without it, a failure above would be ambiguous between "our
	// tier mangled it" and "replaying mangled it" — and the second would be a bug
	// in this file, which is exactly the kind of thing a harness has to be able to
	// rule out rather than argue about.
	let back = Backend::open(&scratch.backend("return")).await?.replay_all(&ours).await?;
	compare_keyspaces("our engine -> upstream", &ours, &back)?;

	println!(
		"round trip: {} keys / {} value bytes survived upstream -> ds+mem:// -> upstream",
		source.len(),
		source.values().map(Vec::len).sum::<usize>()
	);
	Ok(())
}

/// Version lists and tombstones, compared between the two tiers.
///
/// `WORKLOAD` writes `person:one` twice and deletes `person:three`, so the dataset
/// *does* contain a multi-version key and a tombstone — but a whole-keyspace dump
/// cannot show it. A dump reports one value per key, and the `Transactable`
/// surface cannot be asked how many versions a key has: `get` and `scan` take a
/// `version` argument, our tier refuses one with `UnsupportedVersionedQueries`
/// (PLAN.md Phase 1 leaves versioned reads open), and upstream's own memory
/// backend refuses them for the same reason — `surrealdb-kvs-mem`
/// `src/lib.rs:68` rejects `datastore_versioned` at startup rather than accept it
/// and fail every query later.
///
/// So what gets compared is the *observable consequence*, and it is compared as a
/// differential rather than against an absolute expectation: the same op sequence
/// runs against both tiers and the two sets of answers must be equal. That the
/// answers are what they are — a reader pinned at v1 still reads v1 — is recorded
/// in the run's output rather than asserted here, because it is a property of a
/// backend's configuration and not of the contract.
#[tokio::test(flavor = "multi_thread")]
async fn version_lists_and_tombstones_behave_the_same_on_both_tiers() -> anyhow::Result<()> {
	let scratch = Scratch::new("versions")?;
	let upstream = Backend::open(&scratch.backend("upstream")).await?;
	let ours = Backend::open(OURS).await?;
	let (upstream_steps, our_steps) =
		tokio::try_join!(observe_version_history(&upstream), observe_version_history(&ours))?;

	assert_eq!(
		upstream_steps, our_steps,
		"the two tiers disagree about what a multi-version key and a tombstone look like\n\
		 upstream: {upstream_steps:#?}\n ours:     {our_steps:#?}"
	);
	upstream.shutdown().await?;
	ours.shutdown().await?;
	println!(
		"versions: both tiers agree across {} steps:\n  {}",
		upstream_steps.len(),
		upstream_steps.join("\n  ")
	);
	Ok(())
}

/// What a backend says at each step of a version-history op sequence.
///
/// One key taken through: absent → v1 → v2 (a second version of the same key) →
/// absent (tombstone) → v3 (recreated over its own tombstone), with a reader
/// opened *after* v1 and held open throughout, so whether the older version is
/// still reachable is part of the answer rather than an assumption.
///
/// Deliberately comparative. Both tiers are asked the same questions and their
/// answers are returned verbatim; nothing here asserts an absolute expected
/// value. "A reader pinned at v1 still sees v1" is a property of a
/// *configuration* — upstream's RocksDB is unversioned unless asked otherwise —
/// rather than of the `Transactable` contract, so pinning it here would be
/// asserting the oracle's configuration instead of comparing the two tiers.
async fn observe_version_history(backend: &Backend) -> anyhow::Result<Vec<String>> {
	let mut steps = Vec::new();

	steps.push(format!("absent:         {}", read_now(backend).await?));
	apply(backend, Write::Set(b"one")).await?;
	steps.push(format!("after v1:       {}", read_now(backend).await?));

	// Opened after v1 and held open across v2, so its snapshot sits between two
	// versions of the key. This is where the version list makes itself
	// observable; a reader opened before the first write would only re-prove
	// snapshot isolation, which the conformance suite already covers.
	let pinned_reader = backend.txn(TransactionType::Read).await?;
	steps.push(format!("  pinned at v1: {}", read(pinned_reader.as_ref()).await?));

	apply(backend, Write::Set(b"two")).await?;
	steps.push(format!("after v2:       {}", read_now(backend).await?));
	steps.push(format!("  pinned at v1: {}", read(pinned_reader.as_ref()).await?));

	apply(backend, Write::Delete).await?;
	steps.push(format!("after delete:   {}", read_now(backend).await?));

	apply(backend, Write::Set(b"three")).await?;
	steps.push(format!("after recreate: {}", read_now(backend).await?));
	steps.push(format!("  pinned at v1: {}", read(pinned_reader.as_ref()).await?));

	// Cancelled, not leaked: an open transaction pins our tier's GC horizon for
	// the life of the process, which is the hole PROGRESS.md names.
	pinned_reader.cancel().await?;

	// And the recreated key must be visible to a *range* read, not only to a point
	// get: a tier whose tombstones stayed visible in scans would pass every step
	// above and still be wrong.
	let window = bytes(VERSIONED_KEY, &Key::from(VERSIONED_KEY).next());
	let rows = backend.scan(&window).await?;
	steps.push(format!("scan over the key: {} row(s)", rows.len()));
	Ok(steps)
}

/// One write to [`VERSIONED_KEY`], committed in a transaction of its own.
enum Write {
	Set(&'static [u8]),
	Delete,
}

async fn apply(backend: &Backend, write: Write) -> anyhow::Result<()> {
	let txn = backend.txn(TransactionType::Write).await?;
	match write {
		Write::Set(val) => txn.set(Key::from(VERSIONED_KEY), val.to_vec()).await?,
		Write::Delete => txn.del(Key::from(VERSIONED_KEY)).await?,
	};
	Ok(txn.commit().await?)
}

/// A point read in a transaction of its own, so it sees the latest committed
/// state rather than whatever snapshot the caller happens to hold.
async fn read_now(backend: &Backend) -> anyhow::Result<String> {
	let txn = backend.txn(TransactionType::Read).await?;
	let out = read(txn.as_ref()).await?;
	txn.cancel().await?;
	Ok(out)
}

/// The read inside an already-open transaction, whose snapshot is the point.
///
/// `&dyn Transactable` rather than `&Box<dyn Transactable>`: the box is a
/// detail of how the transaction was obtained, and borrowing it is not something
/// a reader of this file should have to notice.
async fn read(txn: &dyn Transactable) -> anyhow::Result<String> {
	let val = txn.get(Key::from(VERSIONED_KEY), None).await?;
	Ok(match val {
		None => "absent".to_owned(),
		Some(bytes) => format!("{} bytes: {}", bytes.len(), show(&bytes)),
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// Comparison
// ─────────────────────────────────────────────────────────────────────────────

/// Compare two keyspaces exactly, naming the first difference.
///
/// Exact: no key, no length and no byte is excused, because in this leg both sides
/// hold the same bytes — one side wrote them, the other read them back.
fn compare_keyspaces(what: &str, expected: &Keyspace, actual: &Keyspace) -> anyhow::Result<()> {
	let missing: Vec<&Vec<u8>> = expected.keys().filter(|key| !actual.contains_key(*key)).collect();
	let extra: Vec<&Vec<u8>> = actual.keys().filter(|key| !expected.contains_key(*key)).collect();
	let changed: Vec<&Vec<u8>> =
		expected.keys().filter(|key| actual.get(*key).is_some_and(|val| *val != expected[*key])).collect();

	if missing.is_empty() && extra.is_empty() && changed.is_empty() {
		return Ok(());
	}
	let mut report = format!("{what}: the two keyspaces differ\n");
	report.push_str(&format!("  {} keys expected, {} found\n", expected.len(), actual.len()));
	for key in missing.iter().take(5) {
		report.push_str(&format!("  missing:  {}\n", show(key)));
	}
	for key in extra.iter().take(5) {
		report.push_str(&format!("  extra:    {}\n", show(key)));
	}
	for key in changed.iter().take(5) {
		// Name the offset, not just the key: "this key differs" leaves the reader
		// to bisect a 123-byte value by hand.
		let expected_val = &expected[*key];
		let actual_val = &actual[*key];
		let at = expected_val
			.iter()
			.zip(actual_val)
			.position(|(want, got)| want != got)
			.unwrap_or(expected_val.len().min(actual_val.len()));
		report.push_str(&format!(
			"  changed:  {} — first differs at byte {at}: expected {}, found {}\n",
			show(key),
			hex(&expected_val[at..at + 1]),
			hex(&actual_val[at..at + 1])
		));
	}
	let hidden = |n: usize| if n > 5 { format!("  … and {} more", n - 5) } else { String::new() };
	report.push_str(&hidden(missing.len().max(extra.len()).max(changed.len())));
	anyhow::bail!(report)
}
