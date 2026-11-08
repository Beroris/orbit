//! Bounded LRU of scaled app icons. Misses are filled by the data thread, and the row paints
//! without an icon until the bytes arrive. Rows never wait on disk.

use std::collections::HashSet;

struct Slot {
    hash: u64,
    size: usize,
    /// `None` records that this app has no icon file, so it isn't requested again.
    pixels: Option<Vec<u8>>,
}

pub struct IconCache {
    slots: Vec<Slot>,
    capacity: usize,
    in_flight: HashSet<(u64, usize)>,
}

impl IconCache {
    pub fn new(capacity: usize) -> Self {
        IconCache { slots: Vec::with_capacity(capacity), capacity, in_flight: HashSet::new() }
    }

    /// Pixels if cached. Does not change the recency order, so painting is read-only.
    pub fn peek(&self, hash: u64, size: usize) -> Option<&[u8]> {
        self.slots
            .iter()
            .find(|slot| slot.hash == hash && slot.size == size)
            .and_then(|slot| slot.pixels.as_deref())
    }

    /// Marks an icon as just used. Returns true if it needs loading.
    pub fn touch(&mut self, hash: u64, size: usize) -> bool {
        if let Some(at) = self.slots.iter().position(|slot| slot.hash == hash && slot.size == size) {
            let slot = self.slots.remove(at);
            self.slots.push(slot);
            return false;
        }
        self.in_flight.insert((hash, size))
    }

    pub fn insert(&mut self, hash: u64, size: usize, pixels: Option<Vec<u8>>) {
        self.in_flight.remove(&(hash, size));
        if self.capacity == 0 {
            return;
        }
        if self.slots.len() == self.capacity {
            self.slots.remove(0);
        }
        self.slots.push(Slot { hash, size, pixels });
    }

    /// Drops everything. Used under memory pressure and on DPI change.
    pub fn clear(&mut self) {
        self.slots = Vec::new();
        self.in_flight = HashSet::new();
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        while self.slots.len() > capacity {
            self.slots.remove(0);
        }
    }

    pub fn bytes(&self) -> usize {
        self.slots.iter().map(|slot| slot.pixels.as_ref().map_or(0, Vec::len)).sum()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut cache = IconCache::new(2);
        cache.insert(1, 32, Some(vec![1]));
        cache.insert(2, 32, Some(vec![2]));
        assert!(!cache.touch(1, 32));
        cache.insert(3, 32, Some(vec![3]));
        assert!(cache.peek(1, 32).is_some());
        assert!(cache.peek(2, 32).is_none());
        assert!(cache.peek(3, 32).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn requests_each_miss_once() {
        let mut cache = IconCache::new(4);
        assert!(cache.touch(9, 32));
        assert!(!cache.touch(9, 32), "already in flight");
        cache.insert(9, 32, None);
        assert!(!cache.touch(9, 32), "known to have no icon");
        assert!(cache.peek(9, 32).is_none());
    }

    #[test]
    fn size_is_part_of_the_identity() {
        let mut cache = IconCache::new(4);
        cache.insert(1, 32, Some(vec![0; 4]));
        assert!(cache.peek(1, 48).is_none());
        assert!(cache.touch(1, 48));
    }

    #[test]
    fn stays_bounded_and_can_be_shed() {
        let mut cache = IconCache::new(3);
        for i in 0..100 {
            cache.insert(i, 32, Some(vec![0; 16]));
        }
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.bytes(), 48);
        cache.set_capacity(1);
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn zero_capacity_stores_nothing() {
        let mut cache = IconCache::new(0);
        cache.insert(1, 32, Some(vec![0; 16]));
        assert_eq!(cache.len(), 0);
    }
}
