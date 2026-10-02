//! What the upstream suite deliberately does not check for us.
//!
//! `surrealdb-kvs-test` marks `snapshot::write_skew_permitted` *ignored* for a
//! backend named `surrealds`, on the grounds that the distributed store is
//! serializable and the test asserts the opposite — snapshot isolation with
//! write skew permitted. Ignoring it is how the suite declines to hold us to
//! serializability, so nothing in the conformance run does. That is a gap in our
//! evidence, and this file is the thing that closes it.
//!
//! Everything here goes through the public seam — the provider, the connection
//! path, `TransactionBuilder` — so it tests the engine as the server gets it.

use surrealdb_cnf::ConfigMap;
use surrealdb_ds::DsBackend;
use surrealdb_kvs::{Error, Transactable, TransactionBuilder, TransactionType};
use surrealdb_kvs_any::Backends;
use tokio_util::sync::CancellationToken;

/// A datastore built the way the server builds one.
async fn ds() -> Box<dyn TransactionBuilder> {
	let mut backends = Backends::community();
	backends.register(DsBackend::new());
	backends
		.new_transaction_builder("ds+mem://", CancellationToken::new(), ConfigMap::empty())
		.await
		.expect("ds+mem:// must construct")
}

/// Two transactions that each read what the other writes cannot both commit.
///
/// This is the write-skew anomaly. Snapshot isolation alone permits it — neither
/// transaction writes a key the other read, so neither has anything to be
/// inconsistent with at the write — and a database that permits it can return a
/// state no serial order of the two transactions produced. Committing `y = 1`
/// after `x = 1` implies both transactions saw the other's write, which they did
/// not.
///
/// The engine is required to refuse the second commit, and to refuse it with a
/// *retryable* error, because a caller that cannot tell a retryable refusal from
/// a definite failure either drops a transaction or replays one that already
/// committed (R-0012).
#[tokio::test]
async fn write_skew_is_prevented() {
	let ds = ds().await;
	let seed = ds.new_transaction(TransactionType::Write).await.unwrap().0;
	seed.set(b"x".into(), b"0".to_vec()).await.unwrap();
	seed.set(b"y".into(), b"0".to_vec()).await.unwrap();
	seed.commit().await.unwrap();

	// tx1 reads y and writes x; tx2 reads x and writes y.
	let (tx1, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	let (tx2, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	assert_eq!(tx1.get(b"y".into(), None).await.unwrap().unwrap(), b"0");
	assert_eq!(tx2.get(b"x".into(), None).await.unwrap().unwrap(), b"0");
	tx1.set(b"x".into(), b"1".to_vec()).await.unwrap();
	tx2.set(b"y".into(), b"1".to_vec()).await.unwrap();

	assert!(tx1.commit().await.is_ok(), "the first committer wins");
	let err = tx2.commit().await.expect_err("the second committer must be refused");
	assert!(
		err.is_retryable(),
		"a refused commit must be retryable, or SDK retry helpers misbehave; got {err}"
	);

	// And the refused transaction wrote nothing.
	let tx = ds.new_transaction(TransactionType::Read).await.unwrap().0;
	assert_eq!(tx.get(b"x".into(), None).await.unwrap().unwrap(), b"1");
	assert_eq!(tx.get(b"y".into(), None).await.unwrap().unwrap(), b"0", "tx2's write must not have landed");
	tx.cancel().await.unwrap();
}

/// The refusal must be about the *read set*, not about who wrote last.
///
/// Two transactions writing different keys, neither having read anything, both
/// commit. Read-set validation that also rejected this would be write-write
/// conflict detection, which the engine does not do and must not: serializability
/// needs the reads checked, and a transaction that read nothing is consistent
/// with anything.
#[tokio::test]
async fn disjoint_writes_both_commit() {
	let ds = ds().await;
	let (a, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	let (b, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	a.set(b"a".into(), b"1".to_vec()).await.unwrap();
	b.set(b"b".into(), b"2".to_vec()).await.unwrap();
	a.commit().await.expect("disjoint writes must not conflict");
	b.commit().await.expect("disjoint writes must not conflict");

	let tx = ds.new_transaction(TransactionType::Read).await.unwrap().0;
	assert!(tx.get(b"a".into(), None).await.unwrap().is_some());
	assert!(tx.get(b"b".into(), None).await.unwrap().is_some());
	tx.cancel().await.unwrap();
}

/// A refused transaction is retried by re-executing it, not by resuming it.
///
/// This is the whole point of making the refusal retryable: the second attempt
/// re-reads, so it sees the committed state and proceeds. An implementation that
/// handed back a transaction whose reads were still pinned to the old snapshot
/// would make retry a no-op.
#[tokio::test]
async fn a_refused_commit_is_retryable_by_reexecution() {
	let ds = ds().await;
	let seed = ds.new_transaction(TransactionType::Write).await.unwrap().0;
	seed.set(b"counter".into(), b"0".to_vec()).await.unwrap();
	seed.commit().await.unwrap();

	/// Advance the counter from `0`, the way a compare-and-swap allocation does.
	async fn advance(tx: &dyn Transactable) -> Result<(), Error> {
		let seen = tx.get(b"counter".into(), None).await.unwrap().unwrap();
		let next = String::from_utf8(seen.clone()).unwrap().parse::<u64>().unwrap() + 1;
		tx.putc(b"counter".into(), next.to_string().into_bytes(), Some(seen)).await.unwrap();
		tx.commit().await
	}

	// Both transactions begin before either commits, so both see `0` and both
	// believe they own the increment.
	let (tx1, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	let (tx2, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	advance(&*tx1).await.expect("the first advancer commits");
	let err = advance(&*tx2).await.expect_err("the second read a counter that moved under it");
	assert!(err.is_retryable(), "a refused commit must be retryable; got {err}");

	// Retry: re-executed, so it re-reads and sees the committed value.
	let (tx3, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	advance(&*tx3).await.expect("a re-executed transaction must be able to commit");

	let tx = ds.new_transaction(TransactionType::Read).await.unwrap().0;
	assert_eq!(tx.get(b"counter".into(), None).await.unwrap().unwrap(), b"2");
	tx.cancel().await.unwrap();
}

/// A read-only transaction's commit never conflicts, whatever it read.
///
/// It has no writes that could follow from its reads, so there is nothing for a
/// concurrent writer to invalidate. Refusing it would make an ordinary read fail
/// because someone else wrote.
#[tokio::test]
async fn a_read_only_commit_is_never_refused() {
	let ds = ds().await;
	let seed = ds.new_transaction(TransactionType::Write).await.unwrap().0;
	seed.set(b"x".into(), b"0".to_vec()).await.unwrap();
	seed.commit().await.unwrap();

	let (reader, _) = ds.new_transaction(TransactionType::Read).await.unwrap();
	reader.get(b"x".into(), None).await.unwrap();

	let (writer, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	writer.set(b"x".into(), b"1".to_vec()).await.unwrap();
	writer.commit().await.unwrap();

	reader.commit().await.expect("a read-only commit must not conflict");
}

/// A range read is a read: a concurrent write inside the range must be seen as a
/// conflict, exactly as a point read would be.
///
/// This is the case a point-only read set would miss, and it is the one a
/// graph traversal or an index rebuild hits.
#[tokio::test]
async fn a_concurrent_write_inside_a_scanned_range_is_a_conflict() {
	let ds = ds().await;
	let seed = ds.new_transaction(TransactionType::Write).await.unwrap().0;
	seed.set(b"k/1".into(), b"0".to_vec()).await.unwrap();
	seed.commit().await.unwrap();

	let (reader, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	let scanned = reader.keys((b"k/".as_slice()..b"k0".as_slice()).into(), u32::MAX, 0, None).await.unwrap();
	assert_eq!(scanned.keys, vec![b"k/1".to_vec()]);

	let (writer, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	writer.set(b"k/2".into(), b"0".to_vec()).await.unwrap();
	writer.commit().await.unwrap();

	let err = reader.commit().await.expect_err("a write inside a scanned range must conflict");
	assert!(err.is_retryable(), "got {err}");
}

/// `getu` is refused, and refused with the error that says so.
///
/// `SELECT … FOR UPDATE` goes through `getu`. The engine has no row locks, so it
/// must say that rather than degrade to a plain read whose conflict guarantee
/// would then be silently dropped — the whole reason the trait's default exists.
#[tokio::test]
async fn locked_reads_are_refused_explicitly() {
	let ds = ds().await;
	let (tx, _) = ds.new_transaction(TransactionType::Write).await.unwrap();
	let err = tx.getu(b"k".into()).await.expect_err("getu must not succeed");
	assert!(
		matches!(err, Error::UnsupportedLockedReads),
		"expected UnsupportedLockedReads, got {err}"
	);
	// Refused on a read-only transaction too, and for the same reason.
	let (reader, _) = ds.new_transaction(TransactionType::Read).await.unwrap();
	assert!(matches!(reader.getu(b"k".into()).await, Err(Error::UnsupportedLockedReads)));
}
