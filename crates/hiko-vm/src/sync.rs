//! Synchronization primitives of the threaded runtime.
//!
//! Normal builds re-export the `std` and `dashmap` types, so this module adds
//! nothing at run time. Builds with `RUSTFLAGS="--cfg loom"` use Loom's
//! primitives instead, so Loom can explore the thread interleavings of the
//! real runtime code (see `threaded_loom.rs`).

#[cfg(not(loom))]
pub(crate) use dashmap::{DashMap, DashSet};
#[cfg(not(loom))]
pub(crate) use std::sync::{Condvar, Mutex, atomic};

#[cfg(loom)]
pub(crate) use loom::sync::{Condvar, Mutex, atomic};
#[cfg(loom)]
pub(crate) use loom_map::{DashMap, DashSet};

/// The part of the `DashMap` API the runtime uses, on one Loom `RwLock`.
///
/// Each operation is atomic, as in `DashMap`. Iteration takes the lock once
/// per entry and does not see a consistent snapshot, like `DashMap`'s
/// shard-by-shard iteration. One lock for the whole map is stricter than
/// `DashMap`'s sharding: holding a guard while writing to the same map
/// deadlocks here for every key, and in `DashMap` only for keys in the same
/// shard. Loom reports such a deadlock, which is a real bug in either case.
#[cfg(loom)]
#[allow(dead_code)] // some methods are used only by `cfg(test)` code
mod loom_map {
    use loom::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
    use std::collections::HashMap;
    use std::hash::Hash;
    use std::ops::{Deref, DerefMut};

    pub(crate) struct DashMap<K, V> {
        map: RwLock<HashMap<K, V>>,
    }

    pub(crate) struct Ref<'a, K, V> {
        guard: RwLockReadGuard<'a, HashMap<K, V>>,
        key: K,
    }

    pub(crate) struct RefMut<'a, K, V> {
        guard: RwLockWriteGuard<'a, HashMap<K, V>>,
        key: K,
    }

    pub(crate) struct Entry<'a, K, V> {
        guard: RwLockWriteGuard<'a, HashMap<K, V>>,
        key: K,
    }

    impl<K: Eq + Hash + Copy, V> DashMap<K, V> {
        pub(crate) fn new() -> Self {
            Self {
                map: RwLock::new(HashMap::new()),
            }
        }

        fn read(&self) -> RwLockReadGuard<'_, HashMap<K, V>> {
            self.map.read().unwrap()
        }

        fn write(&self) -> RwLockWriteGuard<'_, HashMap<K, V>> {
            self.map.write().unwrap()
        }

        pub(crate) fn insert(&self, key: K, value: V) -> Option<V> {
            self.write().insert(key, value)
        }

        pub(crate) fn remove(&self, key: &K) -> Option<(K, V)> {
            self.write().remove_entry(key)
        }

        pub(crate) fn remove_if(&self, key: &K, f: impl FnOnce(&K, &V) -> bool) -> Option<(K, V)> {
            let mut map = self.write();
            match map.get(key) {
                Some(value) if f(key, value) => map.remove_entry(key),
                _ => None,
            }
        }

        pub(crate) fn remove_if_mut(
            &self,
            key: &K,
            f: impl FnOnce(&K, &mut V) -> bool,
        ) -> Option<(K, V)> {
            let mut map = self.write();
            let remove = map.get_mut(key).is_some_and(|value| f(key, value));
            if remove { map.remove_entry(key) } else { None }
        }

        pub(crate) fn get(&self, key: &K) -> Option<Ref<'_, K, V>> {
            let guard = self.read();
            guard.contains_key(key).then_some(Ref { guard, key: *key })
        }

        pub(crate) fn get_mut(&self, key: &K) -> Option<RefMut<'_, K, V>> {
            let guard = self.write();
            guard
                .contains_key(key)
                .then_some(RefMut { guard, key: *key })
        }

        pub(crate) fn entry(&self, key: K) -> Entry<'_, K, V> {
            Entry {
                guard: self.write(),
                key,
            }
        }

        pub(crate) fn contains_key(&self, key: &K) -> bool {
            self.read().contains_key(key)
        }

        pub(crate) fn len(&self) -> usize {
            self.read().len()
        }

        pub(crate) fn is_empty(&self) -> bool {
            self.read().is_empty()
        }

        pub(crate) fn iter(&self) -> impl Iterator<Item = Ref<'_, K, V>> {
            let keys: Vec<K> = self.read().keys().copied().collect();
            keys.into_iter().filter_map(|key| self.get(&key))
        }
    }

    impl<K, V> Ref<'_, K, V> {
        pub(crate) fn key(&self) -> &K {
            &self.key
        }
    }

    impl<K: Eq + Hash, V> Ref<'_, K, V> {
        pub(crate) fn value(&self) -> &V {
            &self.guard[&self.key]
        }
    }

    impl<K: Eq + Hash, V> Deref for Ref<'_, K, V> {
        type Target = V;

        fn deref(&self) -> &V {
            self.value()
        }
    }

    impl<K: Eq + Hash, V> Deref for RefMut<'_, K, V> {
        type Target = V;

        fn deref(&self) -> &V {
            &self.guard[&self.key]
        }
    }

    impl<K: Eq + Hash, V> DerefMut for RefMut<'_, K, V> {
        fn deref_mut(&mut self) -> &mut V {
            self.guard.get_mut(&self.key).unwrap()
        }
    }

    impl<'a, K: Eq + Hash + Copy, V> Entry<'a, K, V> {
        pub(crate) fn or_default(mut self) -> RefMut<'a, K, V>
        where
            V: Default,
        {
            self.guard.entry(self.key).or_default();
            RefMut {
                guard: self.guard,
                key: self.key,
            }
        }
    }

    pub(crate) struct DashSet<K> {
        map: DashMap<K, ()>,
    }

    impl<K: Eq + Hash + Copy> DashSet<K> {
        pub(crate) fn new() -> Self {
            Self {
                map: DashMap::new(),
            }
        }

        pub(crate) fn insert(&self, key: K) -> bool {
            self.map.insert(key, ()).is_none()
        }

        pub(crate) fn remove(&self, key: &K) -> Option<K> {
            self.map.remove(key).map(|(key, ())| key)
        }

        pub(crate) fn contains(&self, key: &K) -> bool {
            self.map.contains_key(key)
        }

        pub(crate) fn len(&self) -> usize {
            self.map.len()
        }

        pub(crate) fn is_empty(&self) -> bool {
            self.map.is_empty()
        }

        pub(crate) fn iter(&self) -> impl Iterator<Item = SetRef<'_, K>> {
            self.map.iter().map(|entry| SetRef(*entry.key(), entry))
        }
    }

    /// A set element. Holds the read lock, like `dashmap::setref::multiple::RefMulti`.
    pub(crate) struct SetRef<'a, K>(K, Ref<'a, K, ()>);

    impl<K> Deref for SetRef<'_, K> {
        type Target = K;

        fn deref(&self) -> &K {
            &self.0
        }
    }
}
