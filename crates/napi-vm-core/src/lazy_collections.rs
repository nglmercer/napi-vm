//! Hash collections that stay unallocated until their first insertion.
//!
//! Object metadata and environment records carry several maps that nearly
//! every instance leaves empty. An inline `HashMap` costs 48 bytes and a
//! `RandomState` seed per construction; these wrappers cost one pointer and
//! nothing until used. They delegate to the standard collections, so lookup
//! semantics and iteration order are unchanged.
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

#[derive(Clone)]
pub(crate) struct LazyMap<K, V>(Option<Box<HashMap<K, V>>>);

impl<K, V> Default for LazyMap<K, V> {
    fn default() -> Self {
        Self(None)
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for LazyMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Hash + Eq, V> LazyMap<K, V> {
    pub(crate) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.as_ref()?.get(key)
    }

    pub(crate) fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.as_mut()?.get_mut(key)
    }

    pub(crate) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.as_ref().is_some_and(|map| map.contains_key(key))
    }

    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.0
            .get_or_insert_with(Default::default)
            .insert(key, value)
    }

    pub(crate) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.as_mut()?.remove(key)
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&K, &mut V) -> bool) {
        if let Some(map) = &mut self.0 {
            map.retain(keep);
        }
    }

    pub(crate) fn extend(&mut self, entries: impl IntoIterator<Item = (K, V)>) {
        let mut entries = entries.into_iter().peekable();
        if entries.peek().is_some() {
            self.0.get_or_insert_with(Default::default).extend(entries);
        }
    }
}

impl<K, V> LazyMap<K, V> {
    pub(crate) fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |map| map.len())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.as_ref().is_none_or(|map| map.is_empty())
    }

    pub(crate) fn clear(&mut self) {
        if let Some(map) = &mut self.0 {
            map.clear();
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.0.iter().flat_map(|map| map.iter())
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
        self.0.iter().flat_map(|map| map.keys())
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().flat_map(|map| map.values())
    }

    /// Remove every entry, yielding them; the allocation is retained.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = (K, V)> + '_ {
        self.0.iter_mut().flat_map(|map| map.drain())
    }
}

#[derive(Clone)]
pub(crate) struct LazySet<T>(Option<Box<HashSet<T>>>);

impl<T> Default for LazySet<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LazySet<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T: Hash + Eq> LazySet<T> {
    pub(crate) fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.as_ref().is_some_and(|set| set.contains(value))
    }

    pub(crate) fn insert(&mut self, value: T) -> bool {
        self.0.get_or_insert_with(Default::default).insert(value)
    }

    pub(crate) fn remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.as_mut().is_some_and(|set| set.remove(value))
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        if let Some(set) = &mut self.0 {
            set.retain(keep);
        }
    }
}

impl<T> LazySet<T> {
    pub(crate) fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |set| set.len())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.as_ref().is_none_or(|set| set.is_empty())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter().flat_map(|set| set.iter())
    }
}
