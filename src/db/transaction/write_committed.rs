use crate::collections::skip_list::skipmap::SkipMap;
use crate::db::key_types::{LSNKey, MemKey, LSN};
use crate::db::no_transaction_db::NoTransactionDB;
use crate::db::{Value, DB};
use crate::memory::MemTable;
use crate::wal::TransactionWAL;
use crate::Result;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLockWriteGuard};

pub struct SnapShot<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK> + 'static,
{
    db: Arc<WriteCommittedDB<UK, M, L>>,
    lsn: LSN,
}

impl<UK, M, L> SnapShot<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK> + 'static,
{
    pub fn range_get(&self, key_start: UK, key_end: UK) -> SkipMap<UK, Value> {
        let key_start = LSNKey::new(key_start, self.lsn);
        let key_end = LSNKey::new(key_end, self.lsn);
        self.db.range_get(&key_start, &key_end).unwrap()
    }

    pub fn get(&self, key: UK) -> Result<Option<Value>> {
        let key = LSNKey::new(key, self.lsn);
        self.db.get(&key)
    }
}

impl<UK, M, L> Drop for SnapShot<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK> + 'static,
{
    fn drop(&mut self) {
        self.db.num_lsn_acquired.fetch_sub(1, Ordering::Release);
    }
}

pub struct WriteBatch<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK> + 'static,
{
    db: Arc<WriteCommittedDB<UK, M, L>>,
    table: SkipMap<LSNKey<UK>, Value>,
    lsn: LSN,
}

impl<UK, M, L> WriteBatch<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK>,
{
    pub fn range_get(&self, key_start: UK, key_end: UK) -> SkipMap<UK, Value> {
        let key_start = LSNKey::new(key_start, self.lsn);
        let key_end = LSNKey::new(key_end, self.lsn);
        let mut kvs = self.db.range_get(&key_start, &key_end).unwrap();
        self.table.range_get(&key_start, &key_end, &mut kvs);
        kvs
    }

    pub fn get(&self, key: UK) -> Result<Option<Value>> {
        let key = LSNKey::new(key, self.lsn);
        match self.table.get_clone(&key) {
            Some(v) => Ok(Some(v)),
            None => self.db.get(&key),
        }
    }

    pub fn set(&mut self, key: UK, value: Value) -> Result<()> {
        let key = LSNKey::new(key, self.lsn);
        self.table.insert(key, value);
        Ok(())
    }

    pub fn remove(&mut self, key: UK) -> Result<()> {
        let key = LSNKey::new(key, self.lsn);
        self.table.insert(key, Value::default());
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<()> {
        std::mem::take(&mut self.table);
        Ok(())
    }
}

impl<UK, M, L> Drop for WriteBatch<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK> + 'static,
{
    fn drop(&mut self) {
        if !self.table.is_empty() {
            let table = std::mem::take(&mut self.table);
            self.db.write_batch(table).unwrap();
        }
        self.db.num_lsn_acquired.fetch_sub(1, Ordering::Release);
    }
}

/// Isolation level: Read committed
///
/// [See `https://github.com/facebook/rocksdb/wiki/WritePrepared-Transactions`]
/// With WriteCommitted write policy, the data is written to the memtable only after the transaction
/// commits. This greatly simplifies the read path as any data that is read by other transactions
/// can be assumed to be committed. This write policy, however, implies that the writes are buffered
/// in memory in the meanwhile. This makes memory a bottleneck for large transactions.
/// The delay of the commit phase in 2PC (two-phase commit) also becomes noticeable since most of
/// the work, i.e., writing to memtable, is done at the commit phase.
/// When the commit of multiple transactions are done in a serial fashion,
/// such as in 2PC implementation of MySQL, the lengthy commit latency
/// becomes a major contributor to lower throughput. Moreover this write policy
/// cannot provide weaker isolation levels, such as READ UNCOMMITTED, that could
/// potentially provide higher throughput for some applications.
pub struct WriteCommittedDB<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK> + 'static,
{
    inner: NoTransactionDB<LSNKey<UK>, UK, M, L>,
    next_lsn: AtomicU64,
    num_lsn_acquired: AtomicU64,
}

impl<UK, M, L> DB<LSNKey<UK>, UK, M> for WriteCommittedDB<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK>,
{
    fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let inner = NoTransactionDB::<LSNKey<UK>, UK, M, L>::open(db_path)?;
        Ok(WriteCommittedDB {
            inner,
            next_lsn: AtomicU64::new(1),
            num_lsn_acquired: AtomicU64::new(0),
        })
    }

    #[inline]
    fn get(&self, key: &LSNKey<UK>) -> Result<Option<Value>> {
        self.inner.get(key)
    }

    #[inline]
    fn set(&self, key: LSNKey<UK>, value: Value) -> Result<()> {
        let guard = self.inner.set_locked(key, value)?;
        self.may_freeze(guard);
        Ok(())
    }

    #[inline]
    fn remove(&self, key: LSNKey<UK>) -> Result<()> {
        let guard = self.inner.remove_locked(key)?;
        self.may_freeze(guard);
        Ok(())
    }

    #[inline]
    fn range_get(
        &self,
        key_start: &LSNKey<UK>,
        key_end: &LSNKey<UK>,
    ) -> Result<SkipMap<UK, Value>> {
        self.inner.range_get(key_start, key_end)
    }
}

impl<UK, M, L> WriteCommittedDB<UK, M, L>
where
    UK: MemKey + From<LSNKey<UK>> + 'static,
    M: MemTable<LSNKey<UK>, UK> + 'static,
    L: TransactionWAL<LSNKey<UK>, UK>,
{
    pub fn snapshot(db: &Arc<Self>) -> SnapShot<UK, M, L> {
        SnapShot {
            db: db.clone(),
            lsn: db.next_lsn.fetch_add(1, Ordering::Release),
        }
    }

    pub fn start_transaction(db: &Arc<Self>) -> WriteBatch<UK, M, L> {
        WriteBatch {
            db: db.clone(),
            table: SkipMap::default(),
            lsn: db.next_lsn.fetch_add(1, Ordering::Release),
        }
    }

    pub fn write_batch(&self, batch: SkipMap<LSNKey<UK>, Value>) -> Result<()> {
        {
            let mut wal_guard = self.inner.wal.lock().unwrap();
            for (key, value) in batch.iter() {
                wal_guard.append(&key, Some(value))?;
            }
        }

        let mut mem_table_guard = self.inner.mut_mem_table.write().unwrap();
        mem_table_guard.merge(batch);

        self.may_freeze(mem_table_guard);
        Ok(())
    }

    fn may_freeze(&self, mem_table_guard: RwLockWriteGuard<M>) {
        if self.num_lsn_acquired.load(Ordering::Acquire) == 0
            && self.inner.should_freeze(mem_table_guard.len())
        {
            self.inner.freeze(mem_table_guard);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::db::key_types::{InternalKey, LSNKey, LSN};
    use crate::db::transaction::write_committed::WriteCommittedDB;
    use crate::db::DB;
    use crate::memory::SkipMapMemTable;
    use crate::wal::lsn_wal::LSNWriteAheadLog;
    use std::sync::Arc;

    #[test]
    fn test_transaction() {
        let temp_dir = tempfile::Builder::new().prefix("txn").tempdir().unwrap();
        let path = temp_dir.path();

        let db =
            Arc::new(
                WriteCommittedDB::<
                    InternalKey,
                    SkipMapMemTable<LSNKey<InternalKey>>,
                    LSNWriteAheadLog,
                >::open(path)
                .unwrap(),
            );
        let mut txn1 = WriteCommittedDB::start_transaction(&db);
        for i in 1..=10i32 {
            txn1.set(Vec::from(i.to_be_bytes()), Vec::from((i + 1).to_be_bytes()))
                .unwrap();
        }

        let key2 = LSNKey::new(Vec::from(2i32.to_be_bytes()), LSN::MAX);
        let value2 = Vec::from(3i32.to_be_bytes());
        assert!(db.get(&key2).unwrap().is_none());
        drop(txn1);
        assert_eq!(db.get(&key2).unwrap().unwrap(), value2);
        let key2 = LSNKey::new(Vec::from(2i32.to_be_bytes()), LSN::MIN);
        assert!(db.get(&key2).unwrap().is_none());

        let snapshot = WriteCommittedDB::snapshot(&db);
        {
            let mut txn2 = WriteCommittedDB::start_transaction(&db);
            txn2.set(
                Vec::from(10i32.to_be_bytes()),
                Vec::from(1000i32.to_be_bytes()),
            )
            .unwrap();
        }
        assert_eq!(
            snapshot.get(Vec::from(10i32.to_be_bytes())).unwrap(),
            Some(Vec::from(11i32.to_be_bytes()))
        );
    }
}