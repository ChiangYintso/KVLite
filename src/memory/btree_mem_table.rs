use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::collections::skip_list::skipmap::SkipMap;
use crate::db::{DBCommand, Key, Value};
use crate::memory::{KeyValue, MemTable};
use crate::Result;

/// Wrapper of `BTreeMap<String, String>`
pub struct BTreeMemTable {
    rw_lock: RwLock<()>,
    inner: BTreeMap<Key, Value>,
}

impl DBCommand for BTreeMemTable {
    fn range_get(&self, key_start: &Key, key_end: &Key, kvs: &mut SkipMap<Key, Value>) {
        let _guard = self.rw_lock.read().unwrap();
        self.inner.get_key_value(key_end);
        for (k, v) in self.inner.range::<Key, _>(key_start..=key_end) {
            kvs.insert(k.clone(), v.clone());
        }
    }

    fn get(&self, key: &Key) -> Result<Option<Value>> {
        let _lock = self.rw_lock.read().unwrap();
        Ok(self.inner.get(key).cloned())
    }

    fn set(&mut self, key: Key, value: Value) -> Result<()> {
        let _lock = self.rw_lock.read().unwrap();
        self.inner.insert(key, value);
        Ok(())
    }

    fn remove(&mut self, key: Key) -> Result<()> {
        let _lock = self.rw_lock.write().unwrap();
        self.inner.insert(key, Key::default());
        Ok(())
    }
}

impl Default for BTreeMemTable {
    fn default() -> Self {
        BTreeMemTable {
            rw_lock: RwLock::default(),
            inner: BTreeMap::default(),
        }
    }
}

impl KeyValue for BTreeMemTable {
    fn len(&self) -> usize {
        let _lock = self.rw_lock.read().unwrap();
        self.inner.len()
    }

    fn kv_iter(&self) -> Box<dyn Iterator<Item = (&Key, &Value)> + '_> {
        let _lock = self.rw_lock.read().unwrap();
        Box::new(self.inner.iter())
    }

    fn first_key(&self) -> Option<&Key> {
        let _lock = self.rw_lock.read().unwrap();
        self.inner.first_key_value().map(|(k, v)| k)
    }

    fn last_key(&self) -> Option<&Key> {
        let _lock = self.rw_lock.read().unwrap();
        self.inner.last_key_value().map(|(k, v)| k)
    }
}

impl MemTable for BTreeMemTable {
    fn merge(&mut self, kvs: SkipMap<Key, Value>) {
        let _guard = self.rw_lock.write().unwrap();
        self.inner.extend(kvs.into_iter());
    }
}

#[cfg(test)]
mod tests {
    use crate::db::DBCommand;
    use crate::memory::{BTreeMemTable, KeyValue};
    use crate::Result;

    #[test]
    fn test_iter() -> Result<()> {
        let mut mem_table = BTreeMemTable::default();
        for i in 0..100i8 {
            mem_table.set(Vec::from(i.to_le_bytes()), Vec::from(i.to_le_bytes()))?;
        }

        for (key, value) in mem_table.kv_iter() {
            assert_eq!(key, value);
        }
        Ok(())
    }
}
