use crate::collections::skip_list::{rand_level, MAX_LEVEL};
use crate::collections::Entry;
use std::alloc::Layout;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

#[repr(C)]
pub struct Node<K: Ord + Default, V: Default> {
    pub entry: Entry<K, V>,
    /// ranges [0, `MAX_LEVEL`]
    level: usize,
    /// the actual size is `level + 1`
    next: [AtomicPtr<Self>; 0],
}

impl<K: Ord + Default, V: Default> Node<K, V> {
    fn head() -> *mut Node<K, V> {
        Self::new_with_level(K::default(), V::default(), MAX_LEVEL)
    }

    fn new_with_level(key: K, value: V, level: usize) -> *mut Node<K, V> {
        let pointers_size = (level + 1) * std::mem::size_of::<AtomicPtr<Self>>();
        let layout = Layout::from_size_align(
            std::mem::size_of::<Self>() + pointers_size,
            std::mem::align_of::<Self>(),
        )
        .unwrap();
        unsafe {
            let node_ptr = std::alloc::alloc(layout) as *mut Self;
            let node = &mut *node_ptr;
            std::ptr::write(&mut node.entry, Entry { key, value });
            std::ptr::write(&mut node.level, level);
            std::ptr::write_bytes(node.next.as_mut_ptr(), 0, level + 1);
            node_ptr
        }
    }

    fn get_layout(&self) -> Layout {
        let pointers_size = (self.level + 1) * std::mem::size_of::<AtomicPtr<Self>>();

        Layout::from_size_align(
            std::mem::size_of::<Self>() + pointers_size,
            std::mem::align_of::<Self>(),
        )
        .unwrap()
    }

    #[inline]
    fn get_next(&self, level: usize) -> *mut Self {
        unsafe { self.next.get_unchecked(level).load(Ordering::Acquire) }
    }

    #[inline]
    fn set_next(&self, level: usize, node: *mut Self) {
        unsafe {
            self.next
                .get_unchecked(level)
                .store(node, Ordering::Release);
        }
    }
}

unsafe fn drop_node<K: Ord + Default, V: Default>(node: *mut Node<K, V>) {
    let layout = (*node).get_layout();
    std::ptr::drop_in_place(node as *mut Node<K, V>);
    std::alloc::dealloc(node as *mut u8, layout);
}

/// Map that allows duplicate keys, based on skip list
///
/// # NOTICE:
///
/// Concurrent insertion is not thread safe but concurrent reading with a
/// single writer is safe.
pub struct MultiSkipMap<K: Ord + Default, V: Default> {
    head: *const Node<K, V>,
    tail: AtomicPtr<Node<K, V>>,
    cur_max_level: AtomicUsize,
    len: AtomicUsize,
}

unsafe impl<K: Ord + Default, V: Default> Send for MultiSkipMap<K, V> {}
unsafe impl<K: Ord + Default, V: Default> Sync for MultiSkipMap<K, V> {}

impl<K: Ord + Default, V: Default> MultiSkipMap<K, V> {
    pub fn new() -> MultiSkipMap<K, V> {
        MultiSkipMap {
            head: Node::head(),
            tail: AtomicPtr::default(),
            cur_max_level: AtomicUsize::default(),
            len: AtomicUsize::default(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// # Safety
    /// node should be null or initialized
    pub unsafe fn node_lt_key(node: *mut Node<K, V>, key: &K) -> bool {
        !node.is_null() && (*node).entry.key.lt(key)
    }

    /// # Safety
    /// node should be null or initialized
    pub unsafe fn node_eq_key(node: *mut Node<K, V>, key: &K) -> bool {
        !node.is_null() && (*node).entry.key.eq(key)
    }

    /// Return the first node `N` whose key is greater or equal than given `key`.
    /// if `prev_nodes` is `Some(...)`, it will be assigned to all the previous nodes of `N`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use kvlite::collections::skip_list::mrsw_skipmap::MultiSkipMap;
    /// let mut skip_map = MultiSkipMap::new();
    /// assert!(skip_map.find_first_ge(&1, None).is_null());
    /// skip_map.insert(3, 3);
    /// assert!(skip_map.find_first_ge(&5, None).is_null());
    /// ```
    pub fn find_first_ge(
        &self,
        key: &K,
        mut prev_nodes: Option<&mut [*const Node<K, V>]>,
    ) -> *mut Node<K, V> {
        let mut level = self.cur_max_level.load(Ordering::Acquire);
        let mut node = self.head;
        loop {
            unsafe {
                let next = (*node).get_next(level);
                if Self::node_lt_key(next, key) {
                    node = next
                } else {
                    if let Some(ref mut p) = prev_nodes {
                        debug_assert_eq!(p.len(), MAX_LEVEL + 1);
                        p[level] = node;
                    }
                    if level == 0 {
                        return next;
                    }
                    level -= 1;
                }
            }
        }
    }

    /// return whether `key` has already exist.
    pub fn insert(&self, key: K, value: V) -> bool {
        let mut prev_nodes = [self.head; MAX_LEVEL + 1];
        let node = self.find_first_ge(&key, Some(&mut prev_nodes));
        let has_key = unsafe { Self::node_eq_key(node, &key) };
        self.insert_after(prev_nodes, key, value);
        has_key
    }

    /// Insert node with `key`, `value` after `prev_nodes`
    fn insert_after(&self, prev_nodes: [*const Node<K, V>; MAX_LEVEL + 1], key: K, value: V) {
        #[cfg(debug_assertions)]
        {
            for (level, prev) in prev_nodes.iter().enumerate() {
                unsafe {
                    debug_assert!((**prev).entry.key.le(&key));
                    Self::node_lt_key((**prev).get_next(level), &key);
                }
            }
        }

        let level = rand_level();
        if level > self.cur_max_level.load(Ordering::Acquire) {
            self.cur_max_level.store(level, Ordering::Release);
        }

        let new_node = Node::new_with_level(key, value, level);
        unsafe {
            if (*(*prev_nodes.get_unchecked(0))).get_next(0).is_null() {
                self.tail.store(new_node, Ordering::Release);
            }
        }

        unsafe {
            for i in 0..=level {
                // set next of new_node first to ensure concurrent read is correct.
                (*new_node).set_next(i, (*(prev_nodes[i])).get_next(i));
                (*(prev_nodes[i])).set_next(i, new_node);
            }
        }

        self.len.fetch_add(1, Ordering::SeqCst);
    }

    /// Remove all the `key` in map, return whether `key` exists
    pub fn remove(&self, key: K) -> bool {
        let mut prev_nodes = [self.head; MAX_LEVEL + 1];
        let mut node = self.find_first_ge(&key, Some(&mut prev_nodes));
        let has_key = unsafe { Self::node_eq_key(node, &key) };
        if has_key {
            unsafe {
                while !node.is_null() && Self::node_eq_key(node, &key) {
                    let next_node = (*node).get_next(0);
                    for i in 0..=(*node).level {
                        (*prev_nodes[i]).set_next(i, (*node).get_next(i))
                    }
                    self.len.fetch_sub(1, Ordering::Release);
                    if next_node.is_null() {
                        self.tail
                            .store(*prev_nodes.get_unchecked(0) as *mut _, Ordering::SeqCst);
                    }
                    drop_node(node);
                    node = next_node;
                }
            }
            true
        } else {
            false
        }
    }

    pub fn iter(&self) -> Iter<K, V> {
        unsafe {
            Iter {
                node: (*self.head).get_next(0),
            }
        }
    }

    /// Get first key-value pair.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use kvlite::collections::skip_list::mrsw_skipmap::MultiSkipMap;
    /// let mut skip_map = MultiSkipMap::new();
    /// assert!(skip_map.first_key_value().is_none());
    ///
    /// skip_map.insert("hello", 2);
    /// skip_map.insert("apple", 1);
    /// let entry = skip_map.first_key_value().unwrap();
    /// assert_eq!(entry.key, "apple");
    /// assert_eq!(entry.value, 1);
    /// ```
    pub fn first_key_value(&self) -> Option<&Entry<K, V>> {
        if self.is_empty() {
            None
        } else {
            unsafe { Some(&(*(*self.head).get_next(0)).entry) }
        }
    }

    /// Get last key-value pair.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use kvlite::collections::skip_list::mrsw_skipmap::MultiSkipMap;
    /// let mut skip_map = MultiSkipMap::new();
    /// assert!(skip_map.last_key_value().is_none());
    ///
    /// skip_map.insert("hello", 2);
    /// skip_map.insert("apple", 1);
    /// let entry = skip_map.last_key_value().unwrap();
    /// assert_eq!(entry.key, "hello");
    /// assert_eq!(entry.value, 2);
    /// ```
    pub fn last_key_value(&self) -> Option<&Entry<K, V>> {
        if self.is_empty() {
            None
        } else {
            Some(unsafe { &(*self.tail.load(Ordering::Acquire)).entry })
        }
    }
}

impl<K: Ord + Default, V: Default> Default for MultiSkipMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Default, V: Default> Drop for MultiSkipMap<K, V> {
    fn drop(&mut self) {
        let mut node = self.head;

        unsafe {
            while !node.is_null() {
                let next_node = (*node).get_next(0);
                drop_node(node as *mut Node<K, V>);
                node = next_node;
            }
        }
    }
}

/// Iteration over the contents of a SkipMap
pub struct Iter<K: Ord + Default, V: Default> {
    node: *const Node<K, V>,
}

impl<K: Ord + Default, V: Default> Iterator for Iter<K, V> {
    type Item = *const Node<K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.node.is_null() {
            None
        } else {
            let n = self.node;
            unsafe {
                self.node = (*self.node).get_next(0);
            }
            Some(n)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::skip_list::mrsw_skipmap::MultiSkipMap;

    #[test]
    fn test_insert() {
        let skip_map: MultiSkipMap<i32, String> = MultiSkipMap::new();
        for i in 0..100 {
            skip_map.insert(i, format!("value{}", i));
            assert_eq!(skip_map.last_key_value().unwrap().key, i);
        }
        debug_assert_eq!(100, skip_map.len());
        for i in 0..100 {
            let node = skip_map.find_first_ge(&i, None);
            unsafe {
                assert_eq!(format!("value{}", i), (*node).entry.value);
            }
        }

        let mut count = 0;
        for node in skip_map.iter() {
            unsafe {
                assert_eq!(format!("value{}", count), (*node).entry.value);
            }
            count += 1;
        }
        assert_eq!(count, skip_map.len());
    }

    #[test]
    fn test_remove() {
        let skip_map: MultiSkipMap<i32, String> = MultiSkipMap::new();
        for i in 0..100 {
            skip_map.insert(i, format!("value{}", i));
        }
        for i in 1..99 {
            assert!(skip_map.remove(i));
        }
        assert_eq!(2, skip_map.len());
        let value = [0, 99];
        for (node, v) in skip_map.iter().zip(value.iter()) {
            unsafe {
                assert_eq!((*node).entry.key, *v);
            }
        }
        skip_map.insert(0, "temp".into());
        assert!(skip_map.remove(0));
        assert_eq!(skip_map.len(), 1);

        assert!(skip_map.remove(99));
        assert!(skip_map.last_key_value().is_none());
        assert!(!skip_map.remove(0));
        assert_eq!(skip_map.len(), 0);
    }

    #[test]
    fn test_first_key_value() {
        let skip_map = MultiSkipMap::new();
        macro_rules! assert_first_key {
            ($k:literal) => {
                assert_eq!(skip_map.first_key_value().unwrap().key, $k);
            };
        }
        assert!(skip_map.first_key_value().is_none());
        skip_map.insert(10, 10);
        assert_first_key!(10);
        skip_map.insert(5, 5);
        assert_first_key!(5);
        skip_map.insert(3, 3);
        assert_first_key!(3);
        skip_map.insert(10, 10);
        assert_first_key!(3);
        skip_map.remove(3);
        assert_first_key!(5);
    }

    #[test]
    fn test_last_key_value() {
        let skip_map = MultiSkipMap::new();

        macro_rules! assert_last_key {
            ($k:literal) => {
                assert_eq!(skip_map.last_key_value().unwrap().key, $k);
            };
        }

        assert!(skip_map.last_key_value().is_none());
        skip_map.insert(10, 10);
        assert_last_key!(10);
        skip_map.insert(5, 5);
        assert_last_key!(10);
        skip_map.insert(13, 13);
        assert_last_key!(13);
        skip_map.insert(14, 14);
        assert_last_key!(14);
        skip_map.remove(14);
        assert_last_key!(13);
    }
}