use core::{hash::BuildHasher, marker::PhantomData};

use crate::Offset;

#[derive(Copy, Clone, Debug)]
#[repr(transparent)]
/// Backwards offset from the end of the serialized buffer
pub(crate) struct CacheOffset(u32);

pub(crate) trait GetCacheKey {
    /// Gets the cachable byte-slice at the start of the buffer
    fn get_cache_key_impl(serialized: &[u8]) -> Option<&[u8]>;
    fn get_cache_key(serialized: &[u8], offset: CacheOffset) -> Option<&[u8]> {
        serialized
            .len()
            .checked_sub(offset.0.try_into().ok()?)
            .and_then(|offset| Self::get_cache_key_impl(&serialized[offset..]))
    }
}

impl<T: ?Sized> From<CacheOffset> for Offset<T> {
    fn from(offset: CacheOffset) -> Self {
        Self {
            offset: offset.0,
            phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> From<Offset<T>> for CacheOffset {
    fn from(offset: Offset<T>) -> Self {
        Self(offset.offset)
    }
}

impl TryFrom<usize> for CacheOffset {
    type Error = <u32 as TryFrom<usize>>::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self)
    }
}

impl From<CacheOffset> for usize {
    fn from(offset: CacheOffset) -> Self {
        offset.0 as usize
    }
}

pub(crate) struct VTable;

impl GetCacheKey for VTable {
    fn get_cache_key_impl(serialized: &[u8]) -> Option<&[u8]> {
        let length = u16::from_le_bytes(serialized.get(..2)?.try_into().ok()?);
        serialized.get(..length as usize)
    }
}

pub(crate) struct ByteVec;

impl GetCacheKey for ByteVec {
    fn get_cache_key_impl(serialized: &[u8]) -> Option<&[u8]> {
        let length = u32::from_le_bytes(serialized.get(..4)?.try_into().ok()?);
        serialized.get(4..4 + length as usize)
    }
}

pub(crate) struct Cache<C> {
    _marker: PhantomData<C>,
    cache: hashbrown::HashTable<CacheOffset>,
    hash_builder: hashbrown::DefaultHashBuilder,
}

impl<C> Default for Cache<C> {
    fn default() -> Self {
        Self {
            _marker: Default::default(),
            cache: Default::default(),
            hash_builder: Default::default(),
        }
    }
}

impl<C> core::fmt::Debug for Cache<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Cache<{}>", core::any::type_name::<C>())
    }
}

impl<C: GetCacheKey> Cache<C> {
    pub(crate) fn hash(&self, serialized_data: &[u8]) -> u64 {
        self.hash_builder.hash_one(serialized_data)
    }

    pub(crate) fn get(
        &mut self,
        serialized_data: &[u8],
        key_hash: u64,
        key: &[u8],
    ) -> Option<CacheOffset> {
        self.cache
            .find(key_hash, |back_offset| {
                C::get_cache_key(serialized_data, *back_offset)
                    .is_some_and(|old_key| old_key == key)
            })
            .copied()
    }

    /// Should only be called if `get` returned `None`
    pub(crate) fn insert(&mut self, key_hash: u64, offset: CacheOffset, serialized_data: &[u8]) {
        self.cache
            .insert_unique(key_hash, CacheOffset(offset.0), |back_offset| {
                C::get_cache_key(serialized_data, *back_offset).map_or_else(
                    || {
                        #[cfg(debug_assertions)]
                        panic!("BUG: It should always be possible to get the cache key");
                        #[cfg(not(debug_assertions))]
                        0
                    },
                    |old_key| self.hash_builder.hash_one(old_key),
                )
            });
    }

    pub(crate) fn clear(&mut self) {
        self.cache.clear();
    }
}

#[cfg(feature = "vector-cache")]
#[derive(Copy, Clone, Debug)]
struct VectorCacheEntry {
    /// Offset of the length prefix
    offset: CacheOffset,
    /// Length of the element data after the length prefix
    data_len: u32,
    /// Alignment mask the element data was written with
    alignment_mask: u32,
}

#[cfg(feature = "vector-cache")]
impl VectorCacheEntry {
    /// Gets the length prefix and element data of the cached vector
    fn key(self, serialized: &[u8]) -> Option<&[u8]> {
        let start = serialized.len().checked_sub(usize::from(self.offset))?;
        serialized.get(start..start.checked_add(4)?.checked_add(self.data_len as usize)?)
    }
}

/// Cache for vectors whose serialized bytes do not depend on their position,
/// keyed by the length prefix and element data.
///
/// Elements of different types can have identical bytes, so a cached vector is
/// only reused when its element data was written with at least the alignment
/// the new vector needs.
#[cfg(feature = "vector-cache")]
#[derive(Default)]
pub(crate) struct VectorCache {
    cache: hashbrown::HashTable<VectorCacheEntry>,
    hash_builder: hashbrown::DefaultHashBuilder,
}

#[cfg(feature = "vector-cache")]
impl core::fmt::Debug for VectorCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VectorCache")
    }
}

#[cfg(feature = "vector-cache")]
impl VectorCache {
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        self.hash_builder.hash_one(key)
    }

    /// `key` is the length prefix followed by the element data.
    pub(crate) fn get(
        &self,
        serialized_data: &[u8],
        key_hash: u64,
        key: &[u8],
        alignment_mask: usize,
    ) -> Option<CacheOffset> {
        self.cache
            .find(key_hash, |entry| {
                entry.alignment_mask as usize >= alignment_mask
                    && entry.key(serialized_data) == Some(key)
            })
            .map(|entry| entry.offset)
    }

    /// Should only be called if `get` returned `None`
    pub(crate) fn insert(
        &mut self,
        key_hash: u64,
        offset: CacheOffset,
        data_len: usize,
        alignment_mask: usize,
        serialized_data: &[u8],
    ) {
        let entry = VectorCacheEntry {
            offset,
            data_len: data_len.try_into().unwrap(),
            alignment_mask: alignment_mask.try_into().unwrap(),
        };
        self.cache.insert_unique(key_hash, entry, |entry| {
            entry.key(serialized_data).map_or_else(
                || {
                    #[cfg(debug_assertions)]
                    panic!("BUG: It should always be possible to get the cache key");
                    #[cfg(not(debug_assertions))]
                    0
                },
                |old_key| self.hash_builder.hash_one(old_key),
            )
        });
    }

    pub(crate) fn clear(&mut self) {
        self.cache.clear();
    }
}
