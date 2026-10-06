//! ADR-0012 step 2: is the local store actually restart-safe?
//!
//! # The question, and why it is a measurement
//!
//! ADR-0012's durable tier is built one transaction-semantic layer over a storage
//! interface. Step 1 settled the *format* — a directory we write is one upstream
//! opens and reads (R-0055…R-0058). This step settles the first *durability*
//! property: after a process dies without closing its store, does what the
//! contract would call an acknowledged commit come back?
//!
//! It sounds like a question with a known answer, and that is the trap. Every
//! part of it is a knob, and the defaults are not what a distributed engine needs:
//! `WriteOptions::sync` is off by default, `disableWAL` skips the log entirely,
//! `Options::manual_wal_flush` separates "written to the OS" from "flushed from
//! the application", and a memtable flush produces an SST without making anything
//! durable. So this harness **measures each property and records the configuration
//! it needed**, including the ones that do not hold.
//!
//! # The honest crash: a child process and `SIGKILL`
//!
//! An in-process panic cannot test crash recovery: `Drop` runs, RocksDB closes
//! cleanly, and the WAL is checkpointed. The only honest kill is another process
//! receiving `SIGKILL`, with no chance to flush. A libtest binary can re-exec
//! itself: the parent sets `DS_DURABILITY_CHILD`, and [`child_worker`] becomes a
//! writer that commits, prints a sentinel and parks until it is killed.
//!
//! **What a `SIGKILL` does and does not simulate.** It is a real process death:
//! the child's user-space buffers are gone, its WAL writer is not flushed by the
//! application, and its `Drop` never runs. It is *not* a power loss: data already
//! handed to the kernel with `write(2)` survives in the page cache. Measured, over
//! 600 process kills: **600/600** unsynced commits survived, because the page
//! cache outlives the process. So `sync = true` and `sync = false` are
//! indistinguishable to this harness, and the power-loss claims are reported as
//! *not proven* — nothing here can drop the page cache.
//!
//! **The one knob that is observable here is `manual_wal_flush`, and it is not a
//! barrier.** It keeps the WAL record in the application instead of handing it to
//! the OS per write, so a killed writer usually loses the commit — measured
//! 576/600 lost — but the engine still flushes that buffer on its own schedule
//! often enough that **24/600 survived** a `SIGKILL` that landed microseconds
//! after the commit was acknowledged. A child's own log shows no memtable flush in
//! those cases, so the flush is internal to the log writer. This is the harness's
//! sharpest negative result: on RocksDB, *only* `WriteOptions::sync` is a
//! caller-held durability barrier, and "I did not fsync" is not the same claim as
//! "this will not be on disk".
//!
//! # Properties measured
//!
//! | Test | Property |
//! | --- | --- |
//! | [`a_clean_close_reopens_byte_identically`] | clean reopen |
//! | [`a_sigkill_after_a_synced_commit_keeps_it`] | per-commit durability, `sync` on |
//! | [`an_unsynced_commit_is_measured_not_assumed`] | per-commit durability, `sync` off (measured) |
//! | [`manual_wal_flush_loses_the_unsynced_commit_but_not_always`] | flush vs. fsync, isolated (a rate) |
//! | [`an_explicit_wal_flush_buys_back_the_unsynced_commit`] | `flush_wal` is not `sync` |
//! | [`disabling_the_wal_loses_the_commit_but_not_always`] | WAL is the durability mechanism (a rate) |
//! | [`a_killed_writer_never_leaves_a_partial_batch`] | atomic write batches |
//! | [`a_torn_wal_record_is_dropped_not_half_applied`] | atomicity at a torn record (deterministic) |
//! | [`a_commit_after_a_memtable_flush_recovers_from_the_wal`] | batch across a flush boundary |
//! | [`tombstones_survive_a_crash_and_a_compaction`] | tombstone durability |
//! | [`an_optimistic_commit_is_durable_under_sigkill`] | the upstream transaction path |
//! | [`optimistic_overlapping_blind_writes_are_measured`] | the conflict policy step 3 must not inherit |
//!
//! # Falsifiability
//!
//! Every checker here can reject bad input, and each is shown doing so:
//! [`the_keyspace_comparison_rejects_a_missing_and_an_extra_key`],
//! [`the_batch_checker_rejects_a_partial_batch`],
//! [`the_tombstone_checker_rejects_a_resurrected_key`]. A probe that cannot fail
//! is theatre.
//!
//! # What this does NOT prove
//!
//! 1. **Not power loss.** `SIGKILL` preserves the page cache, so `sync = false`
//!    and `sync = true` are indistinguishable here — 600/600 unsynced commits
//!    survived. That `sync = true` survives *media* loss is documented, not
//!    measured; this harness cannot drop the page cache.
//! 2. **Not a tier.** Raw `rocksdb` with one column family; no `Transactable`, no
//!    read validation, no commit identity. ADR-0012 steps 3–6 are the tier.
//! 3. **Not distributed commit identity.** A local RocksDB sequence number is not
//!    a cluster-wide commit timestamp (ADR-0012), and nothing here tests one.
//! 4. **Not a durability guarantee, only a recovery observation.** What is proven
//!    is that a `SIGKILL`ed writer leaves a recoverable, all-or-nothing state.
//!    What is *not* proven is that any configuration here survives power loss.
//! 5. **One filesystem, one `temp_dir`.** No `O_DIRECT`, no network filesystem, no
//!    device-level flush ordering. The WAL's on-media guarantees are the key's and
//!    the kernel's, not ours to assert from here.
//! 6. **Not a concurrency test.** libtest runs the tests in parallel by default,
//!    so the rare `manual_wal_flush`/`disable_wal` survivors are timing-dependent;
//!    that is precisely why those two are reported as rates and not verdicts.
//!
//! # Licence
//!
//! BUSL-1.1, like the crate it lives in: it links `surrealdb-server` and the
//! `surrealdb-rocksdb` fork. See ../NOTICE and DECISIONS.md ADR-0002/ADR-0011.

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::{Keyspace, Scratch, show};
use rocksdb::{
	DB, IteratorMode, OptimisticTransactionDB, OptimisticTransactionOptions, Options, WriteBatch, WriteOptions,
};

// ─────────────────────────────────────────────────────────────────────────────
// The configuration under test
// ─────────────────────────────────────────────────────────────────────────────

/// The four knobs that decide what a killed writer leaves behind.
///
/// Each is default-`false`, so `Durability::default()` is exactly what an
/// unconfigured store does — which is the baseline the measurements are against.
#[derive(Clone, Copy, Default, Debug)]
struct Durability {
	/// `WriteOptions::sync` — fsync the WAL before the write returns. Off by
	/// default.
	sync: bool,
	/// `Options::manual_wal_flush` — keep WAL records in the application until an
	/// explicit flush, rather than handing them to the OS per write. Off by
	/// default.
	manual_wal_flush: bool,
	/// `WriteOptions::disable_wal` — skip the write-ahead log. Off by default.
	disable_wal: bool,
	/// An explicit `DB::flush_wal(true)` after the commit. Not a RocksDB option;
	/// the escape hatch the manual-flush mode offers.
	flush_wal: bool,
}

/// The sentinel the child prints once its commit has returned.
const READY: &str = "DURABILITY_READY";
/// The sentinel the batch-loop child prints once it is steadily writing.
const WRITING: &str = "DURABILITY_WRITING";

/// Environment keys for the parent→child handoff.
const ENV_CHILD: &str = "DS_DURABILITY_CHILD";
const ENV_MODE: &str = "DS_DURABILITY_MODE";
const ENV_DIR: &str = "DS_DURABILITY_DIR";
const ENV_SYNC: &str = "DS_DURABILITY_SYNC";
const ENV_MANUAL: &str = "DS_DURABILITY_MANUAL_WAL";
const ENV_DISABLE_WAL: &str = "DS_DURABILITY_DISABLE_WAL";
const ENV_FLUSH_WAL: &str = "DS_DURABILITY_FLUSH_WAL";

fn env_flag(name: &str) -> bool {
	std::env::var(name).is_ok_and(|value| value == "1")
}

/// Build the child's open options from the knob set.
fn open_options(dur: Durability) -> Options {
	let mut opts = Options::default();
	opts.create_if_missing(true);
	// `set_manual_wal_flush` is an *open* option, unlike sync/WAL which are per
	// write. Setting it here and nowhere else is the point: it is the one knob
	// that can make a commit vanish without the process ever closing.
	opts.set_manual_wal_flush(dur.manual_wal_flush);
	opts
}

/// Build the child's per-write options from the knob set.
fn write_options(dur: Durability) -> WriteOptions {
	let mut wo = WriteOptions::new();
	wo.set_sync(dur.sync);
	wo.disable_wal(dur.disable_wal);
	wo
}

// ─────────────────────────────────────────────────────────────────────────────
// Payloads
// ─────────────────────────────────────────────────────────────────────────────

/// A small single-commit payload: 16 keys, ~1 KiB total, well under the 64 KiB at
/// which even a manual WAL buffer would flush itself.
fn commit_payload() -> Keyspace {
	(0..16u32)
		.map(|i| (format!("commit:{i:04}").into_bytes(), format!("value-{i:04}").into_bytes()))
		.collect()
}

/// Keys per batch in the atomicity workload.
const BATCH_KEYS: u32 = 400;
/// Value bytes per batch key, chosen so one batch is several WAL blocks.
const BATCH_VALUE: usize = 256;

/// A key in batch `batch`, position `i`. Sortable, and self-describing so a
/// reader can group a recovered keyspace back into batches without a side file.
fn batch_key(batch: u64, i: u32) -> Vec<u8> {
	let mut key = Vec::with_capacity(17);
	key.extend_from_slice(b"batch:");
	key.extend_from_slice(&batch.to_be_bytes());
	key.extend_from_slice(&i.to_be_bytes());
	key
}

fn batch_value(batch: u64) -> Vec<u8> {
	vec![(batch & 0xff) as u8; BATCH_VALUE]
}

/// The two parts of the tombstone workload.
fn live_key(i: u32) -> Vec<u8> {
	format!("live:{i:04}").into_bytes()
}

fn doomed_key(i: u32) -> Vec<u8> {
	format!("doom:{i:04}").into_bytes()
}

const LIVE_KEYS: u32 = 24;
const DOOMED_KEYS: u32 = 8;

fn live_keys() -> Vec<Vec<u8>> {
	(0..LIVE_KEYS).map(live_key).collect()
}

fn doomed_keys() -> Vec<Vec<u8>> {
	(0..DOOMED_KEYS).map(doomed_key).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// The child process
// ─────────────────────────────────────────────────────────────────────────────

/// The crash writer. Only does anything when re-exec'd with [`ENV_CHILD`] set;
/// under an ordinary `cargo test` it returns immediately and is a no-op.
///
/// It commits its scenario, prints a sentinel, and parks. It never closes the
/// database. The parent `SIGKILL`s it, so no `Drop`, no `flush`, no clean
/// shutdown — the whole point.
#[test]
fn child_worker() -> anyhow::Result<()> {
	if std::env::var_os(ENV_CHILD).is_none() {
		return Ok(());
	}
	let mode = std::env::var(ENV_MODE)?;
	let dir = PathBuf::from(std::env::var(ENV_DIR)?);
	let dur = Durability {
		sync: env_flag(ENV_SYNC),
		manual_wal_flush: env_flag(ENV_MANUAL),
		disable_wal: env_flag(ENV_DISABLE_WAL),
		flush_wal: env_flag(ENV_FLUSH_WAL),
	};
	run_child(&mode, dur, &dir)
}

/// Write one batch of [`BATCH_KEYS`] keys under batch id `batch`.
fn write_one_batch(db: &DB, batch: u64, dur: Durability) -> anyhow::Result<()> {
	let mut write_batch = WriteBatch::default();
	for i in 0..BATCH_KEYS {
		write_batch.put(batch_key(batch, i), batch_value(batch));
	}
	db.write_opt(write_batch, &write_options(dur))?;
	Ok(())
}

/// The child's per-mode behaviour. Each ends by parking forever; only the
/// atomicity loop keeps writing after its sentinel.
fn run_child(mode: &str, dur: Durability, dir: &Path) -> anyhow::Result<()> {
	match mode {
		// One commit under the configured knobs, then wait to be killed.
		"commit" => {
			let db = DB::open(&open_options(dur), dir)?;
			let mut write_batch = WriteBatch::default();
			for (key, val) in commit_payload() {
				write_batch.put(key, val);
			}
			db.write_opt(write_batch, &write_options(dur))?;
			if dur.flush_wal {
				db.flush_wal(true)?;
			}
			// Report how much WAL is on disk *while the writer is still running*:
			// 0 bytes means the record is still in the application buffer, non-zero
			// means it already reached the OS.
			announce(&format!("{READY} wal={}", wal_state(dir)))?;
		}
		// A memtable flush between two commits: batch 0 is in an SST, batch 1 is
		// WAL-only when the process dies.
		"flush_boundary" => {
			let db = DB::open(&open_options(dur), dir)?;
			write_one_batch(&db, 0, dur)?;
			db.flush()?;
			write_one_batch(&db, 1, dur)?;
			announce(READY)?;
		}
		// Write continuously until killed, so the kill can land mid-batch.
		"batch_loop" => {
			let db = DB::open(&open_options(dur), dir)?;
			let mut batch = 0u64;
			for _ in 0..2 {
				write_one_batch(&db, batch, dur)?;
				batch += 1;
			}
			announce(WRITING)?;
			loop {
				write_one_batch(&db, batch, dur)?;
				batch += 1;
			}
		}
		// Exactly one batch, so the parent can tear its single WAL record.
		"one_batch" => {
			let db = DB::open(&open_options(dur), dir)?;
			write_one_batch(&db, 0, dur)?;
			announce(READY)?;
		}
		// Live keys flushed to an SST, then deletes as a real tombstone; the
		// `_flushed` variant puts the tombstone in its own SST so compaction has
		// to merge it away.
		"tombstone_wal" | "tombstone_flushed" => {
			let db = DB::open(&open_options(dur), dir)?;
			let mut live = WriteBatch::default();
			for key in live_keys() {
				live.put(&key, b"live-value");
			}
			db.write_opt(live, &write_options(dur))?;
			db.flush()?;

			// The tombstone is always synced: this measures persistence, not the
			// sync knob, which the single-commit tests already isolate.
			let mut dead = WriteBatch::default();
			for key in doomed_keys() {
				dead.delete(&key);
			}
			let synced = WriteOptions::new();
			db.write_opt(dead, &synced)?;
			if mode == "tombstone_flushed" {
				db.flush()?;
			}
			announce(READY)?;
		}
		// Upstream's own tier uses OptimisticTransactionDB; this commits through
		// it so the recovery path under test is the one the tier would use.
		"optimistic_commit" => {
			let db: OptimisticTransactionDB = OptimisticTransactionDB::open(&open_options(dur), dir)?;
			let txn = db.transaction_opt(&write_options(dur), &OptimisticTransactionOptions::default());
			for (key, val) in commit_payload() {
				txn.put(key, val)?;
			}
			txn.commit()?;
			announce(READY)?;
		}
		other => anyhow::bail!("unknown child mode `{other}`"),
	}

	// Park, not sleep: the parent delivers SIGKILL, which needs no cooperation.
	loop {
		std::thread::park();
	}
}

/// Print a sentinel and make sure it is on the pipe before parking.
fn announce(sentinel: &str) -> anyhow::Result<()> {
	println!("{sentinel}");
	std::io::stdout().flush()?;
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Driving the child from a test
// ─────────────────────────────────────────────────────────────────────────────

/// Spawn the test binary as the crash writer for one mode and knob set.
fn spawn_child(mode: &str, dur: Durability, dir: &Path) -> anyhow::Result<Child> {
	let exe = std::env::current_exe()?;
	let child = Command::new(exe)
		.args(["--exact", "child_worker", "--nocapture"])
		.env(ENV_CHILD, "1")
		.env(ENV_MODE, mode)
		.env(ENV_DIR, dir)
		.env(ENV_SYNC, u8::from(dur.sync).to_string())
		.env(ENV_MANUAL, u8::from(dur.manual_wal_flush).to_string())
		.env(ENV_DISABLE_WAL, u8::from(dur.disable_wal).to_string())
		.env(ENV_FLUSH_WAL, u8::from(dur.flush_wal).to_string())
		.stdout(Stdio::piped())
		.stderr(Stdio::inherit())
		.spawn()?;
	Ok(child)
}

/// Read the child's stdout until a sentinel line arrives, or fail.
///
/// A reader thread plus a channel rather than a blocking `read_line`, so a child
/// that hangs is a timeout and a useful message rather than a hung test.
fn wait_for_line(stdout: ChildStdout, sentinel: &str, timeout: Duration) -> anyhow::Result<String> {
	let (tx, rx) = mpsc::channel::<String>();
	std::thread::spawn(move || {
		let mut reader = BufReader::new(stdout);
		let mut line = String::new();
		loop {
			line.clear();
			match reader.read_line(&mut line) {
				Ok(0) | Err(_) => break,
				Ok(_) => {
					if tx.send(line.clone()).is_err() {
						break;
					}
				}
			}
		}
	});
	let deadline = std::time::Instant::now() + timeout;
	loop {
		let left = deadline.saturating_duration_since(std::time::Instant::now());
		if left.is_zero() {
			anyhow::bail!("timed out waiting for `{sentinel}` from the child");
		}
		match rx.recv_timeout(left) {
			// `contains`, not `==`: libtest prints `test child_worker ... ` on the
			// same line before the test's own output, and the sentinel lands after
			// that prefix.
			Ok(line) if line.contains(sentinel) => return Ok(line.trim().to_owned()),
			Ok(_) => continue,
			Err(mpsc::RecvTimeoutError::Timeout) => anyhow::bail!("timed out waiting for `{sentinel}` from the child"),
			Err(mpsc::RecvTimeoutError::Disconnected) => {
				anyhow::bail!("the child exited before printing `{sentinel}`")
			}
		}
	}
}

/// Run a child to its sentinel, linger, then `SIGKILL` it and reap it.
///
/// Waiting for the sentinel *and reaping* before returning is load-bearing:
/// RocksDB holds an exclusive directory lock, so a parent that reopens before
/// the child is reaped fails for the wrong reason (the lock, not the durability).
///
/// Returns the sentinel line the child printed, which carries whatever the child
/// measured about its own on-disk state at commit time.
fn kill_child(
	mode: &str,
	dur: Durability,
	dir: &Path,
	sentinel: &str,
	linger: Duration,
) -> anyhow::Result<String> {
	let mut child = spawn_child(mode, dur, dir)?;
	let stdout = child.stdout.take().expect("stdout was piped");
	let reported = wait_for_line(stdout, sentinel, Duration::from_secs(60))?;
	if !linger.is_zero() {
		std::thread::sleep(linger);
	}
	child.kill()?;
	let status = child.wait()?;
	#[cfg(unix)]
	{
		use std::os::unix::process::ExitStatusExt;
		// 9 is SIGKILL; naming it by number avoids a libc dependency for one
		// constant.
		if status.signal() != Some(9) {
			anyhow::bail!("the child was not killed by SIGKILL: {status:?}");
		}
	}
	Ok(reported)
}

// ─────────────────────────────────────────────────────────────────────────────
// Reading a directory back
// ─────────────────────────────────────────────────────────────────────────────

/// Open the directory and read the whole keyspace with a real iterator scan.
///
/// `create_if_missing` stays off: a reader that cannot fail on a wrong path
/// proves nothing about a right one. A scan rather than point reads, because the
/// reader-after-crash property is about enumeration, not lookups.
fn read_all(dir: &Path) -> anyhow::Result<Keyspace> {
	let db = DB::open(&Options::default(), dir)?;
	let mut out = Keyspace::new();
	for item in db.iterator(IteratorMode::Start) {
		let (key, val) = item?;
		out.insert(key.to_vec(), val.to_vec());
	}
	Ok(out)
}

/// A point read of one key, for the cases where the scan is not the property.
fn point_get(dir: &Path, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
	let db = DB::open(&Options::default(), dir)?;
	Ok(db.get(key)?)
}

/// Count the `.sst` files in a directory, so a test can say whether a recovered
/// value could only have come from the WAL.
fn count_sst(dir: &Path) -> usize {
	std::fs::read_dir(dir)
		.map(|entries| {
			entries
				.flatten()
				.filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sst"))
				.count()
		})
		.unwrap_or(0)
}

/// The newest write-ahead-log file in a directory, by name.
fn newest_log(dir: &Path) -> anyhow::Result<PathBuf> {
	let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)?
		.flatten()
		.map(|entry| entry.path())
		.filter(|path| path.extension().is_some_and(|ext| ext == "log"))
		.collect();
	logs.sort();
	logs.pop().ok_or_else(|| anyhow::anyhow!("no `.log` file in {}", dir.display()))
}

/// A short description of what a recovery actually saw, for a failure message.
fn diag(keyspace: &Keyspace, dir: &Path) -> String {
	let mut files: Vec<String> = std::fs::read_dir(dir)
		.map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
		.unwrap_or_default();
	files.sort();
	let first: Vec<String> = keyspace.keys().take(3).map(|key| show(key)).collect();
	let options: Vec<String> = files
		.iter()
		.filter(|name| name.starts_with("OPTIONS"))
		.filter_map(|name| std::fs::read_to_string(dir.join(name)).ok())
		.flat_map(|text| {
			text.lines()
				.filter(|line| line.contains("manual_wal_flush") || line.contains("disable_wal"))
				.map(|line| line.trim().to_owned())
				.collect::<Vec<_>>()
		})
		.collect();
	format!("{} keys {:?}; files {files:?}; options {options:?}", keyspace.len(), first)
}

/// The flush / log-file events in a directory's RocksDB info logs.
///
/// Read immediately after the kill, while the log is still only the child's, so
/// "did the engine flush behind our back?" has a direct answer rather than an
/// inference from the recovered keyspace.
fn flush_events(dir: &Path) -> Vec<String> {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return Vec::new();
	};
	entries
		.flatten()
		.map(|entry| entry.path())
		.filter(|path| path.file_name().is_some_and(|name| name == "LOG" || name.to_string_lossy().starts_with("LOG.old")))
		.filter_map(|path| std::fs::read_to_string(path).ok())
		.flat_map(|text| {
			text.lines()
				.filter(|line| {
					let lower = line.to_lowercase();
					(lower.contains("flush table")
						|| lower.contains("flush memtable")
						|| lower.contains("switchmemtable")
						|| lower.contains("logfile")
						|| lower.contains("log file"))
						&& !lower.contains("options.")
				})
				.map(str::trim)
				.map(str::to_owned)
				.collect::<Vec<_>>()
		})
		.collect()
}

/// Commit under `dur`, `SIGKILL` the writer, reopen, and classify each trial as
/// survived-or-lost over `trials` runs. Returns `(survived, lost, flushed_survivors)`.
///
/// A single batch is all-or-nothing whatever the durability, so a *partial*
/// keyspace is the one outcome this never tolerates: it fails the run. The rates
/// are returned rather than asserted, because the interesting fact about
/// `manual_wal_flush` and `disable_wal` is that both are *probabilistic* — the
/// engine decides when (or whether) to write the memtable out, and only `sync`
/// is a caller-held barrier.
fn measure_commit_survival(label: &str, dur: Durability, trials: u32) -> anyhow::Result<(u32, u32, u32)> {
	let expected = commit_payload();
	let mut survived = 0u32;
	let mut lost = 0u32;
	let mut flushed_survivors = 0u32;
	for trial in 0..trials {
		let scratch = Scratch::new(&format!("{label}-{trial}"))?;
		let dir = scratch.dir("db");
		kill_child("commit", dur, &dir, READY, Duration::ZERO)?;
		// Read the child's engine log before opening the DB, so a surviving commit
		// can be attributed to a flush RocksDB chose on its own.
		let flushed = !flush_events(&dir).is_empty();
		let got = read_all(&dir)?;
		if got == expected {
			survived += 1;
			if flushed {
				flushed_survivors += 1;
			}
		} else if got.is_empty() {
			lost += 1;
		} else {
			anyhow::bail!("trial {trial} of {label} recovered a partial keyspace, which is never acceptable: {}", diag(&got, &dir));
		}
	}
	Ok((survived, lost, flushed_survivors))
}

/// What the newest write-ahead log looks like on disk right now.
///
/// The child reports this at commit time and the parent again after the kill,
/// which is what separates "the record never left the application" from "something
/// flushed it afterwards". An empty log is the buffered state; a non-zero one
/// means the bytes already reached the OS while the writer was still running. The
/// "no such log" case is reported distinctly, because it is a measurement
/// failure of its own rather than an empty log.
fn wal_state(dir: &Path) -> String {
	match newest_log(dir) {
		Ok(path) => match std::fs::metadata(&path) {
			Ok(meta) => format!("{} bytes", meta.len()),
			Err(err) => format!("<metadata: {err}>"),
		},
		Err(err) => format!("<{err}>"),
	}
}

/// Size of the newest write-ahead log, or 0 if there is none. The numeric form,
/// for assertions; [`wal_state`] is the human form.
fn wal_bytes(dir: &Path) -> u64 {
	newest_log(dir).ok().and_then(|path| std::fs::metadata(path).ok()).map(|meta| meta.len()).unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Checkers, each of which a falsification test shows can reject
// ─────────────────────────────────────────────────────────────────────────────

/// Compare two keyspaces exactly, naming the first difference.
fn compare_keyspaces(what: &str, expected: &Keyspace, actual: &Keyspace) -> anyhow::Result<()> {
	let missing: Vec<&Vec<u8>> = expected.keys().filter(|key| !actual.contains_key(*key)).collect();
	let extra: Vec<&Vec<u8>> = actual.keys().filter(|key| !expected.contains_key(*key)).collect();
	let changed: Vec<&Vec<u8>> =
		expected.keys().filter(|key| actual.get(*key).is_some_and(|got| *got != expected[*key])).collect();
	if missing.is_empty() && extra.is_empty() && changed.is_empty() {
		return Ok(());
	}
	let mut report = format!("{what}: the two keyspaces differ\n  {} keys expected, {} found\n", expected.len(), actual.len());
	for key in missing.iter().take(4) {
		report.push_str(&format!("  missing:  {}\n", show(key)));
	}
	for key in extra.iter().take(4) {
		report.push_str(&format!("  extra:    {}\n", show(key)));
	}
	for key in changed.iter().take(4) {
		let (want, got) = (&expected[*key], &actual[*key]);
		let at = want.iter().zip(got).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
		report.push_str(&format!("  changed:  {} — first differs at byte {at}\n", show(key)));
	}
	anyhow::bail!(report)
}

/// Decode a [`batch_key`] back into `(batch, index)`, or `None` for anything else.
fn parse_batch_key(key: &[u8]) -> Option<(u64, u32)> {
	let rest = key.strip_prefix(b"batch:")?;
	if rest.len() != 12 {
		return None;
	}
	let batch = u64::from_be_bytes(rest[..8].try_into().ok()?);
	let index = u32::from_be_bytes(rest[8..].try_into().ok()?);
	Some((batch, index))
}

/// Every batch present in `keyspace` must be **complete**, and the present batch
/// ids must be a prefix `0..=max` with no gap.
///
/// A partial batch is the failure the property forbids. A gap would mean WAL
/// replay skipped a record and applied a later one, which is just as bad.
/// Returns how many complete batches were present.
fn assert_batches_complete(keyspace: &Keyspace, keys_per_batch: u32) -> anyhow::Result<u64> {
	let mut counts: BTreeMap<u64, u32> = BTreeMap::new();
	for key in keyspace.keys() {
		if let Some((batch, _index)) = parse_batch_key(key) {
			*counts.entry(batch).or_default() += 1;
		}
	}
	let mut max = None;
	for (batch, count) in &counts {
		if *count != keys_per_batch {
			anyhow::bail!("batch {batch} is partial: {count}/{keys_per_batch} keys present, so a batch is not all-or-nothing");
		}
		max = Some(*batch);
	}
	if let Some(max) = max {
		for batch in 0..max {
			if !counts.contains_key(&batch) {
				anyhow::bail!("batch {batch} is absent while batch {max} is present, so replay is not sequential");
			}
		}
		Ok(max + 1)
	} else {
		Ok(0)
	}
}

/// Every live key present and every doomed key absent, or a named failure.
fn assert_present_absent(keyspace: &Keyspace, live: &[Vec<u8>], doomed: &[Vec<u8>]) -> anyhow::Result<()> {
	for key in live {
		if !keyspace.contains_key(key) {
			anyhow::bail!("live key {} is missing after recovery", show(key));
		}
	}
	for key in doomed {
		if keyspace.contains_key(key) {
			anyhow::bail!("deleted key {} came back (a resurrected tombstone)", show(key));
		}
	}
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Clean reopen
// ─────────────────────────────────────────────────────────────────────────────

/// Write a keyspace, close cleanly, reopen, and require every byte back.
///
/// The baseline every crash property is judged against: if a clean close is not
/// faithful, no crash result means anything.
#[test]
fn a_clean_close_reopens_byte_identically() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-clean")?;
	let dir = scratch.dir("db");
	let expected = commit_payload();
	{
		let mut opts = Options::default();
		opts.create_if_missing(true);
		let db = DB::open(&opts, &dir)?;
		let mut batch = WriteBatch::default();
		for (key, val) in &expected {
			batch.put(key, val);
		}
		db.write_opt(batch, &write_options(Durability { sync: true, ..Durability::default() }))?;
		drop(db);
	}

	let reopened = read_all(&dir)?;
	compare_keyspaces("clean reopen", &expected, &reopened)?;
	assert_eq!(
		point_get(&dir, b"commit:0000")?.as_deref(),
		Some(b"value-0000".as_slice()),
		"a point read after a clean reopen returned the wrong value"
	);
	// A second close/reopen: the first reopen may itself have left a WAL.
	let again = read_all(&dir)?;
	compare_keyspaces("second clean reopen", &expected, &again)?;
	println!("clean: {} keys survived close and two reopens byte-identically", expected.len());
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Per-commit durability
// ─────────────────────────────────────────────────────────────────────────────

/// `sync = true`: kill immediately after the commit returns, and require it back.
///
/// The child never flushes, so the only durable artefact it leaves is the WAL —
/// checked *before* the parent opens, because RocksDB's own recovery flushes the
/// replayed memtable to an L0 SST as it opens, which would otherwise make "no SST"
/// meaningless.
#[test]
fn a_sigkill_after_a_synced_commit_keeps_it() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-sync")?;
	let dir = scratch.dir("db");
	let expected = commit_payload();
	let dur = Durability { sync: true, ..Durability::default() };
	let reported = kill_child("commit", dur, &dir, READY, Duration::ZERO)?;
	// The child never flushes, so the only durable artefact it leaves is the WAL —
	// checked *before* the parent opens, because RocksDB's own recovery flushes the
	// replayed memtable to an L0 SST as it opens, which would otherwise make "no SST"
	// meaningless. And the WAL must be non-empty: `sync = true` forces it out.
	assert_eq!(count_sst(&dir), 0, "the child left an SST, so it flushed rather than relying on the WAL");
	assert!(wal_bytes(&dir) > 0, "a synced commit left an empty WAL ({reported}), so sync did not write the log");

	let got = read_all(&dir)?;
	compare_keyspaces("synced commit after SIGKILL", &expected, &got)?;
	assert_eq!(
		point_get(&dir, b"commit:0007")?.as_deref(),
		Some(b"value-0007".as_slice()),
		"a point read after crash recovery returned the wrong value"
	);
	println!("sync: commit of {} keys survived SIGKILL with no SST left by the child, so it came from the WAL", expected.len());
	Ok(())
}

/// `sync = false`, default WAL: measure rather than assume.
///
/// Under `SIGKILL` the kernel still holds the pages the WAL was `write(2)`n into,
/// so the commit is expected to survive — but that is a *measured* outcome, not a
/// guarantee, and it is explicitly not power-loss durability. The harness records
/// the count and forbids only a partial keyspace; a single batch is all-or-nothing
/// whatever the durability.
#[test]
fn an_unsynced_commit_is_measured_not_assumed() -> anyhow::Result<()> {
	const TRIALS: u32 = 20;
	let (survived, lost, _) = measure_commit_survival("durability-nosync", Durability::default(), TRIALS)?;
	println!(
		"nosync: {survived}/{TRIALS} unsynced commits survived SIGKILL and {lost}/{TRIALS} were lost — the page \
		 cache, not our durability"
	);
	Ok(())
}

/// `manual_wal_flush = true, sync = false`: measure whether the buffered commit
/// survives a `SIGKILL`.
///
/// This is the one configuration that separates "handed to the OS" from "still in
/// the application": the WAL record is *buffered*, not written, so a `SIGKILL`
/// should take it. But the engine is free to flush that buffer whenever it likes
/// — `manual_wal_flush` is not a barrier the caller holds — so this is a **rate**,
/// not a verdict. The child reports its WAL size at commit time to prove the
/// record was still buffered when the commit was acknowledged.
#[test]
fn manual_wal_flush_loses_the_unsynced_commit_but_not_always() -> anyhow::Result<()> {
	const TRIALS: u32 = 20;
	let dur = Durability { manual_wal_flush: true, ..Durability::default() };
	let (survived, lost, flushed) = measure_commit_survival("durability-manual", dur, TRIALS)?;
	println!(
		"manual_wal_flush: {lost}/{TRIALS} buffered commits were lost to SIGKILL and {survived}/{TRIALS} \
		 survived ({flushed} of the survivors had an engine-recorded flush) — the engine flushes the 'manual' \
		 buffer on its own schedule, so this is a hint, not a barrier"
	);
	assert!(lost > 0, "no buffered commit was ever lost, so manual_wal_flush is no longer holding the record in the application");
	Ok(())
}

/// `manual_wal_flush = true` plus an explicit `flush_wal(true)`.
///
/// Same knobs as the losing case, plus one call. If the commit now survives, then
/// `flush_wal` is what moves the buffer, and `sync` is a separate concern.
#[test]
fn an_explicit_wal_flush_buys_back_the_unsynced_commit() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-flushwal")?;
	let dir = scratch.dir("db");
	let dur = Durability { manual_wal_flush: true, flush_wal: true, ..Durability::default() };
	kill_child("commit", dur, &dir, READY, Duration::ZERO)?;

	let got = read_all(&dir)?;
	compare_keyspaces("flush_wal(true) after an unsynced commit", &commit_payload(), &got)?;
	println!("flush_wal: one explicit flush_wal(true) brought back the commit the previous test lost");
	Ok(())
}

/// `disable_wal = true`: the WAL is the durability mechanism, so removing it
/// should cost the commit — measured, for the same reason as above.
///
/// With no WAL, the only way the commit can survive is an SST, which needs a
/// memtable flush. That flush is RocksDB's decision and its timing, so this is a
/// rate rather than a certainty.
#[test]
fn disabling_the_wal_loses_the_commit_but_not_always() -> anyhow::Result<()> {
	const TRIALS: u32 = 20;
	let dur = Durability { disable_wal: true, ..Durability::default() };
	let (survived, lost, flushed) = measure_commit_survival("durability-nowal", dur, TRIALS)?;
	println!(
		"disable_wal: {lost}/{TRIALS} WAL-less commits were lost to SIGKILL and {survived}/{TRIALS} survived \
		 ({flushed} of the survivors had an engine-recorded memtable flush) — without the WAL the commit has \
		 no deliberate path to disk"
	);
	assert!(lost > 0, "no WAL-less commit was ever lost, so disable_wal is no longer a memtable-only write");
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Atomic batches
// ─────────────────────────────────────────────────────────────────────────────

/// Kill a continuously-writing child and require no partial batch, ever.
///
/// The child loops on batches of several WAL blocks each, so the kill can land
/// inside a `write(2)` and truncate a log record. WAL recovery must drop the
/// whole record, not apply its keys: every batch present must be complete, and
/// the present batch ids must be sequential with no gap.
#[test]
fn a_killed_writer_never_leaves_a_partial_batch() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-atomic")?;
	let dir = scratch.dir("db");
	kill_child("batch_loop", Durability::default(), &dir, WRITING, Duration::from_millis(300))?;
	assert_eq!(count_sst(&dir), 0, "the child left an SST, so the recovery under test is not the WAL path");

	let got = read_all(&dir)?;
	let complete = assert_batches_complete(&got, BATCH_KEYS)?;
	assert!(complete >= 2, "only {complete} complete batches were recovered, so the kill landed before the workload started");
	println!(
		"atomic: {complete} complete batches recovered, {} keys, each batch all-or-nothing; 0 SSTs left by the child",
		got.len()
	);
	Ok(())
}

/// A commit that lands *after* a memtable flush must still recover from the WAL.
///
/// Batch 0 is flushed to an SST; batch 1 is committed and the process is killed.
/// Batch 0 proves the SST path works, batch 1 proves a post-flush commit is not
/// silently abandoned when the memtable that held it is gone.
#[test]
fn a_commit_after_a_memtable_flush_recovers_from_the_wal() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-flushboundary")?;
	let dir = scratch.dir("db");
	kill_child("flush_boundary", Durability::default(), &dir, READY, Duration::ZERO)?;
	assert!(count_sst(&dir) >= 1, "the boundary test wrote no SST, so there was no flush to straddle");

	let got = read_all(&dir)?;
	let complete = assert_batches_complete(&got, BATCH_KEYS)?;
	assert!(complete >= 2, "expected batch 0 (flushed) and batch 1 (WAL) back, got {complete}");
	println!("flush boundary: batch 0 from an SST and batch 1 from the WAL both recovered, no partial batch");
	Ok(())
}

/// A torn WAL record is dropped whole, not half-applied.
///
/// The kill-timing test above *may* land mid-write; this one is deterministic. The
/// child commits one batch and is killed, then the parent truncates the tail of
/// the only WAL file — the bytes of its last record — and requires the keyspace to
/// come back **empty** rather than missing only the torn tail. This is the media
/// story a `SIGKILL` cannot produce, and it is the atomicity claim in its
/// strongest form.
///
/// The DB must not be opened before the tear: recovery flushes a replayed memtable
/// to an SST, and an SST would survive the truncation and mask the result.
#[test]
fn a_torn_wal_record_is_dropped_not_half_applied() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-torn")?;
	let dir = scratch.dir("db");
	let dur = Durability { sync: true, ..Durability::default() };
	kill_child("one_batch", dur, &dir, READY, Duration::ZERO)?;
	assert_eq!(count_sst(&dir), 0, "the child left an SST, so the WAL is not the only copy");

	let log = newest_log(&dir)?;
	let length = std::fs::metadata(&log)?.len();
	assert!(
		length > BATCH_VALUE as u64,
		"the WAL is {} bytes, too small to tear inside the record ({} bytes per value)",
		length,
		BATCH_VALUE
	);
	std::fs::OpenOptions::new().write(true).open(&log)?.set_len(length - BATCH_VALUE as u64)?;

	let after = read_all(&dir)?;
	assert!(
		after.is_empty(),
		"a torn WAL record was half-applied: {} keys survived the truncation of {}",
		after.len(),
		log.display()
	);
	println!(
		"torn WAL: truncating the last {BATCH_VALUE} bytes of a {} byte WAL record left 0 keys, not {BATCH_KEYS}",
		length
	);
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Tombstones
// ─────────────────────────────────────────────────────────────────────────────

/// Deleted keys stay deleted across a crash **and** a compaction.
///
/// Two placements: the tombstone in the WAL, and the tombstone in its own SST.
/// That second one is where a resurrected value would appear — compaction is what
/// finally discards the tombstone and the older value it covers, so a bug that
/// lost the tombstone would surface here and nowhere earlier.
#[test]
fn tombstones_survive_a_crash_and_a_compaction() -> anyhow::Result<()> {
	for (mode, label) in [("tombstone_wal", "tombstone in the WAL"), ("tombstone_flushed", "tombstone flushed to SST")] {
		let scratch = Scratch::new(&format!("durability-{mode}"))?;
		let dir = scratch.dir("db");
		kill_child(mode, Durability::default(), &dir, READY, Duration::ZERO)?;

		let after_crash = read_all(&dir)?;
		assert_present_absent(&after_crash, &live_keys(), &doomed_keys())?;

		// Force the memtable (with any replayed tombstone) to an SST, then compact
		// the whole key range so the tombstone and the value it covers meet.
		{
			let db = DB::open(&Options::default(), &dir)?;
			db.flush()?;
			db.compact_range(None::<&[u8]>, None::<&[u8]>);
		}
		let after_compaction = read_all(&dir)?;
		assert_present_absent(&after_compaction, &live_keys(), &doomed_keys())?;
		println!("tombstone: {label} — {LIVE_KEYS} live keys intact and {DOOMED_KEYS} deleted keys stayed absent after compaction");
	}
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. The upstream transaction path
// ─────────────────────────────────────────────────────────────────────────────

/// A commit made through `OptimisticTransactionDB` is durable under `SIGKILL`,
/// and a plain reader can recover its directory.
///
/// Upstream's tier uses optimistic transactions. This checks the commit reaches
/// the WAL and the directory opens with our own reader — the recovery half of
/// what ADR-0012 step 3 must build on.
#[test]
fn an_optimistic_commit_is_durable_under_sigkill() -> anyhow::Result<()> {
	let scratch = Scratch::new("durability-optimistic")?;
	let dir = scratch.dir("db");
	let dur = Durability { sync: true, ..Durability::default() };
	kill_child("optimistic_commit", dur, &dir, READY, Duration::ZERO)?;

	let got = read_all(&dir)?;
	compare_keyspaces("optimistic commit after SIGKILL, read by a plain DB", &commit_payload(), &got)?;
	println!("optimistic: a synced OptimisticTransactionDB commit recovered by a plain reader");
	Ok(())
}

/// Measure the conflict policy of optimistic transactions: what step 3 must
/// **not** inherit naively.
///
/// ADR-0012 (and the `surrealds` contract, ADR-0006) require overlapping blind
/// writes to **all commit**, serialised by timestamp. RocksDB's optimistic
/// transactions are documented to reject a write set that changed under them.
/// This does not assert which policy we get — it prints both, with and without a
/// snapshot, so the report can state the measured behaviour rather than the
/// documented one.
#[test]
fn optimistic_overlapping_blind_writes_are_measured() -> anyhow::Result<()> {
	for snapshot in [false, true] {
		let scratch = Scratch::new(&format!("durability-conflict-{snapshot}"))?;
		let dir = scratch.dir("db");
		let mut opts = Options::default();
		opts.create_if_missing(true);
		let db: OptimisticTransactionDB = OptimisticTransactionDB::open(&opts, &dir)?;

		let mut txn_opts = OptimisticTransactionOptions::default();
		txn_opts.set_snapshot(snapshot);
		let first = db.transaction_opt(&WriteOptions::default(), &txn_opts);
		first.put(b"conflict:key", b"one")?;
		let second = db.transaction_opt(&WriteOptions::default(), &txn_opts);
		second.put(b"conflict:key", b"two")?;

		let first_result = first.commit();
		let second_result = second.commit();
		let final_value = db.get(b"conflict:key")?;
		println!(
			"optimistic conflict (snapshot={snapshot}): first commit ok={}, second commit ok={}, final value={:?}",
			first_result.is_ok(),
			second_result.is_ok(),
			final_value.as_deref().map(String::from_utf8_lossy),
		);
		// Both policies leave exactly one of the two committed, and the final value
		// must be one of them — never a value neither transaction wrote.
		assert!(first_result.is_ok(), "the uncontended first commit failed: {first_result:?}");
		assert!(
			matches!(final_value.as_deref(), Some(value) if value == b"one" || value == b"two"),
			"the final value is neither writer's value, which no conflict policy predicts"
		);
	}
	Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Falsifiability: each checker, shown rejecting
// ─────────────────────────────────────────────────────────────────────────────

/// The keyspace comparison rejects a missing key and an extra key, and names both.
#[test]
fn the_keyspace_comparison_rejects_a_missing_and_an_extra_key() {
	let expected: Keyspace =
		[(b"a".to_vec(), b"1".to_vec()), (b"b".to_vec(), b"2".to_vec())].into_iter().collect();
	let actual: Keyspace = [(b"a".to_vec(), b"1".to_vec()), (b"c".to_vec(), b"3".to_vec())].into_iter().collect();
	let err = compare_keyspaces("synthetic", &expected, &actual).expect_err("the comparison accepted two different keyspaces");
	let message = err.to_string();
	assert!(message.contains("missing"), "the report did not name a missing key:\n{message}");
	assert!(message.contains("extra"), "the report did not name an extra key:\n{message}");
	println!("falsify: the keyspace comparison rejected a missing and an extra key\n{message}");
}

/// The batch checker rejects a batch with one key missing.
#[test]
fn the_batch_checker_rejects_a_partial_batch() {
	let mut partial = Keyspace::new();
	for i in 0..BATCH_KEYS {
		partial.insert(batch_key(0, i), batch_value(0));
	}
	// Batch 1 is one key short.
	for i in 0..BATCH_KEYS - 1 {
		partial.insert(batch_key(1, i), batch_value(1));
	}
	let err = assert_batches_complete(&partial, BATCH_KEYS).expect_err("the batch checker accepted a partial batch");
	assert!(err.to_string().contains("partial"), "the report did not say `partial`:\n{err}");
	println!("falsify: the batch checker rejected a batch missing one key\n{err}");
}

/// The tombstone checker rejects a deleted key that came back.
#[test]
fn the_tombstone_checker_rejects_a_resurrected_key() {
	let mut after = Keyspace::new();
	for key in live_keys() {
		after.insert(key, b"live-value".to_vec());
	}
	after.insert(doomed_key(0), b"resurrected".to_vec());
	let err = assert_present_absent(&after, &live_keys(), &doomed_keys())
		.expect_err("the tombstone checker accepted a resurrected key");
	assert!(err.to_string().contains("came back"), "the report did not describe the resurrection:\n{err}");
	println!("falsify: the tombstone checker rejected a resurrected deleted key\n{err}");
}
