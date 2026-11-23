//! A JS `Map` with string keys, as Pele's dbs use it: entries in the order
//! their keys were first set, and setting a key again keeps its place.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
    index: BTreeMap<String, usize>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        OrderedMap {
            entries: Vec::new(),
            index: BTreeMap::new(),
        }
    }
}

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `map.size`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `map.has(key)`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// Where `key` is in insertion order.
    pub fn position(&self, key: &str) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// `map.get(key)`.
    pub fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        match self.index.get(key) {
            Some(&i) => Some(&mut self.entries[i].1),
            None => None,
        }
    }

    /// `map.set(key, value)`: a key already there keeps its place.
    pub fn insert(&mut self, key: &str, value: V) {
        match self.index.get(key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(key.into(), self.entries.len());
                self.entries.push((key.into(), value));
            }
        }
    }

    /// `map.delete(key)`.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let i = self.index.remove(key)?;
        let (_, v) = self.entries.remove(i);
        for slot in self.index.values_mut() {
            if *slot > i {
                *slot -= 1;
            }
        }
        Some(v)
    }

    /// The entry at a place in insertion order.
    pub fn at(&self, i: usize) -> (&str, &V) {
        let (k, v) = &self.entries[i];
        (k, v)
    }

    pub fn at_mut(&mut self, i: usize) -> &mut V {
        &mut self.entries[i].1
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    /// `map.clear()`.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }
}
