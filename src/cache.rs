//! Small in-memory page cache. Acts as a *lookup table*: we deliberately keep
//! it tiny (one megabyte by default) and rely on the SSD for the rest. This is
//! the inverse of a buffer pool that tries to hold the working set in RAM.

use std::collections::HashMap;
use std::sync::Arc;

use crate::storage::page::{Page, PageId};

pub const DEFAULT_CAPACITY: usize = 256;

pub struct PageCache {
    capacity: usize,
    map: HashMap<PageId, Entry>,
    clock: u64,
    hits: u64,
    misses: u64,
}

struct Entry {
    page: Arc<Page>,
    last: u64,
}

impl PageCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::new(),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    pub fn get(&mut self, id: PageId) -> Option<Arc<Page>> {
        self.clock += 1;
        if let Some(e) = self.map.get_mut(&id) {
            e.last = self.clock;
            self.hits += 1;
            Some(Arc::clone(&e.page))
        } else {
            self.misses += 1;
            None
        }
    }

    pub fn put(&mut self, id: PageId, page: Arc<Page>) {
        self.clock += 1;
        if !self.map.contains_key(&id) && self.map.len() >= self.capacity {
            self.evict_lru();
        }
        self.map.insert(
            id,
            Entry {
                page,
                last: self.clock,
            },
        );
    }

    pub fn invalidate(&mut self, id: PageId) {
        self.map.remove(&id);
    }

    fn evict_lru(&mut self) {
        if let Some((&victim, _)) = self.map.iter().min_by_key(|(_, e)| e.last) {
            self.map.remove(&victim);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::PageKind;

    fn page(id: PageId) -> Arc<Page> {
        Arc::new(Page::new(id, PageKind::Node))
    }

    #[test]
    fn hit_then_miss_counters() {
        let mut c = PageCache::new(4);
        c.put(1, page(1));
        c.put(2, page(2));
        assert!(c.get(1).is_some());
        assert!(c.get(2).is_some());
        assert!(c.get(99).is_none());
        assert_eq!(c.hits(), 2);
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn lru_eviction_drops_oldest() {
        let mut c = PageCache::new(2);
        c.put(1, page(1));
        c.put(2, page(2));
        // touch 1 so 2 becomes LRU
        let _ = c.get(1);
        c.put(3, page(3));
        assert!(c.get(1).is_some(), "1 should survive (recent)");
        assert!(c.get(2).is_none(), "2 should have been evicted");
        assert!(c.get(3).is_some(), "3 just inserted");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn invalidate_drops_entry() {
        let mut c = PageCache::new(4);
        c.put(7, page(7));
        c.invalidate(7);
        assert!(c.get(7).is_none());
    }
}
