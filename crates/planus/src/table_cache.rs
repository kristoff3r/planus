//! Deduplication of tables and vectors of offsets by content.
//!
//! The serialized form of a table or vector depends on where it is written,
//! because offsets are stored relative to the position of the field holding
//! them. The cache therefore never looks at the written bytes. Instead the
//! table or vector is encoded a second time into a key buffer, as if it were
//! written at the fixed position [`KEY_POSITION`]. Each relative offset in that
//! encoding is `KEY_POSITION - field_index - target`, which identifies the
//! absolute target position, and every other value is encoded exactly as in
//! the buffer: scalars and floats by their bit patterns, structs by their
//! bytes. Two values with equal keys are thus written as identical bytes, and
//! the earlier one can be referenced instead of writing the later one.
//!
//! Keys are kept in an arena, so a hash match is always confirmed by comparing
//! the complete key.

use alloc::vec::Vec;
use core::{hash::BuildHasher, marker::PhantomData, mem::MaybeUninit};

use crate::{Builder, Offset, Primitive, VectorWrite};

/// The position a key is encoded at, counted from the end of the buffer like
/// every other position. It lies above any position a real buffer can reach,
/// so encoding an offset relative to it never wraps.
pub(crate) const KEY_POSITION: u32 = u32::MAX;

/// Distinguishes the keys of the kinds of values that share a cache.
#[derive(Copy, Clone)]
#[repr(u8)]
pub(crate) enum KeyKind {
    Table = 0,
    Vector = 1,
}

#[derive(Copy, Clone)]
struct Entry {
    hash: u64,
    key_start: usize,
    key_len: usize,
    offset: u32,
}

#[derive(Default)]
pub(crate) struct TableCache {
    entries: hashbrown::HashTable<Entry>,
    keys: Vec<u8>,
    hash_builder: hashbrown::DefaultHashBuilder,
    /// End of the stored keys, where the key being built starts.
    pending_start: usize,
}

impl core::fmt::Debug for TableCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TableCache")
            .field("entries", &self.entries.len())
            .field("key_bytes", &self.keys.len())
            .finish()
    }
}

impl TableCache {
    /// Starts a new key. The key holds the kind of value, its alignment mask
    /// and whatever the caller appends with [`Self::extend_key`] and
    /// [`Self::reserve_key`].
    pub(crate) fn start_key(&mut self, kind: KeyKind, alignment_mask: usize) {
        self.keys.truncate(self.pending_start);
        self.pending_start = self.keys.len();
        self.keys.push(kind as u8);
        // The mask is `2^k - 1`, so `k` identifies it.
        self.keys.push(alignment_mask.count_ones() as u8);
    }

    pub(crate) fn extend_key(&mut self, bytes: &[u8]) {
        self.keys.extend_from_slice(bytes);
    }

    /// Appends `len` bytes to the key and returns them for the caller to
    /// initialize.
    pub(crate) fn reserve_key(&mut self, len: usize) -> &mut [MaybeUninit<u8>] {
        let start = self.keys.len();
        self.keys.resize(start + len, 0);
        let bytes = &mut self.keys[start..];
        // SAFETY: `MaybeUninit<u8>` has the layout of `u8`, and the bytes are
        // initialized. The callers only ever write initialized bytes through the
        // returned slice, so the `Vec<u8>` stays fully initialized.
        unsafe { &mut *(bytes as *mut [u8] as *mut [MaybeUninit<u8>]) }
    }

    /// Looks up the key built since [`Self::start_key`]. On a hit the key is
    /// dropped and the earlier offset returned. On a miss the key is kept until
    /// [`Self::insert`] stores it, and its hash is returned.
    pub(crate) fn get(&mut self) -> Result<u32, u64> {
        let key = &self.keys[self.pending_start..];
        let hash = self.hash_builder.hash_one(key);
        let keys = &self.keys;
        let found = self.entries.find(hash, |entry| {
            entry.hash == hash && &keys[entry.key_start..entry.key_start + entry.key_len] == key
        });
        match found {
            Some(entry) => {
                let offset = entry.offset;
                self.keys.truncate(self.pending_start);
                Ok(offset)
            }
            None => Err(hash),
        }
    }

    /// Stores the key built since [`Self::start_key`], which [`Self::get`] did
    /// not find, as written at `offset`.
    pub(crate) fn insert(&mut self, hash: u64, offset: u32) {
        let entry = Entry {
            hash,
            key_start: self.pending_start,
            key_len: self.keys.len() - self.pending_start,
            offset,
        };
        self.entries.insert_unique(hash, entry, |entry| entry.hash);
        self.pending_start = self.keys.len();
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.keys.clear();
        self.pending_start = 0;
    }
}

/// Returns the offset of a vector holding `values`: an earlier equal one if the
/// builder deduplicates vectors of offsets, otherwise the one `write` writes.
#[inline]
pub(crate) fn dedup_vector<T: VectorWrite<P>, P: Primitive>(
    builder: &mut Builder,
    values: &[T::Value],
    alignment_mask: usize,
    write: impl FnOnce(&mut Builder) -> Offset<[P]>,
) -> Offset<[P]> {
    if P::IS_OFFSET
        && let Some(cache) = &mut builder.table_cache
    {
        cache.start_key(KeyKind::Vector, alignment_mask);
        cache.extend_key(&(values.len() as u32).to_le_bytes());
        let bytes = cache.reserve_key(T::STRIDE * values.len());
        // SAFETY: `bytes` holds `values.len()` elements of `T::STRIDE` bytes.
        unsafe { T::write_values(values, bytes.as_mut_ptr(), KEY_POSITION) };
        return match cache.get() {
            Ok(offset) => Offset {
                offset,
                phantom: PhantomData,
            },
            Err(hash) => {
                let offset = write(builder);
                if let Some(cache) = &mut builder.table_cache {
                    cache.insert(hash, offset.offset);
                }
                offset
            }
        };
    }
    write(builder)
}

#[cfg(test)]
mod tests {
    use crate::{Builder, Cursor, Offset, Primitive, WriteAsPrimitive, table_writer::TableWriter};

    /// Writes a table with a `u32`, an optional string and an `f32` field.
    fn table(builder: &mut Builder, a: u32, s: Option<Offset<str>>, f: f32) -> Offset<()> {
        let mut table_writer = TableWriter::<10>::default();
        table_writer.write_entry::<u32>(0);
        if s.is_some() {
            table_writer.write_entry::<Offset<str>>(1);
        }
        table_writer.write_entry::<f32>(2);
        unsafe {
            table_writer.finish_with_offset(builder, |object_writer| {
                object_writer.write::<_, _, 4>(&a);
                if let Some(s) = &s {
                    object_writer.write::<_, _, 4>(s);
                }
                object_writer.write::<_, _, 4>(&f);
            })
        }
    }

    fn cached_builder() -> Builder {
        let mut builder = Builder::new();
        builder.set_table_cache(true);
        builder
    }

    #[test]
    fn disabled_by_default() {
        let mut builder = Builder::new();
        assert!(!builder.table_cache_enabled());
        let first = table(&mut builder, 1, None, 1.0);
        let second = table(&mut builder, 1, None, 1.0);
        assert_ne!(first, second);
    }

    #[test]
    fn equal_tables_share_an_offset() {
        let mut builder = cached_builder();
        let first = table(&mut builder, 1, None, 1.0);
        let len = builder.len();
        let second = table(&mut builder, 1, None, 1.0);
        assert_eq!(first, second);
        assert_eq!(builder.len(), len);
        assert_ne!(first, table(&mut builder, 2, None, 1.0));
    }

    #[test]
    fn floats_compare_by_bits() {
        let mut builder = cached_builder();
        let zero = table(&mut builder, 0, None, 0.0);
        assert_ne!(zero, table(&mut builder, 0, None, -0.0));
        let nan = table(&mut builder, 0, None, f32::NAN);
        assert_eq!(nan, table(&mut builder, 0, None, f32::NAN));
    }

    #[test]
    fn absent_field_differs_from_default() {
        let mut builder = cached_builder();
        let absent = table(&mut builder, 0, None, 0.0);
        let mut table_writer = TableWriter::<10>::default();
        table_writer.write_entry::<u32>(0);
        table_writer.write_entry::<f32>(2);
        table_writer.write_entry::<u32>(1);
        let present: Offset<()> = unsafe {
            table_writer.finish_with_offset(&mut builder, |object_writer| {
                object_writer.write::<_, _, 4>(&0u32);
                object_writer.write::<_, _, 4>(&0.0f32);
                object_writer.write::<_, _, 4>(&0u32);
            })
        };
        assert_ne!(absent, present);
    }

    #[test]
    fn children_compare_by_position() {
        let mut builder = cached_builder();
        let a = builder.create_string("a");
        let b = builder.create_string("b");
        let with_a = table(&mut builder, 0, Some(a), 0.0);
        let with_b = table(&mut builder, 0, Some(b), 0.0);
        assert_ne!(with_a, with_b);
        // Writing more data moves the position the next table would be written
        // at, which changes its relative offset to `a` but not its key.
        builder.create_string("padding");
        assert_eq!(with_a, table(&mut builder, 0, Some(a), 0.0));
        assert_eq!(with_b, table(&mut builder, 0, Some(b), 0.0));
    }

    struct Unaligned8;

    // SAFETY: 8 bytes with 4-byte alignment, written by the impl below.
    unsafe impl Primitive for Unaligned8 {
        const ALIGNMENT: usize = 4;
        const SIZE: usize = 8;
    }

    struct UnalignedU64(u64);

    impl WriteAsPrimitive<Unaligned8> for UnalignedU64 {
        fn write<const N: usize>(&self, cursor: Cursor<'_, N>, _buffer_position: u32) {
            cursor.assert_size().finish(self.0.to_le_bytes());
        }
    }

    #[test]
    fn alignment_is_part_of_the_key() {
        let mut builder = cached_builder();
        let write = |builder: &mut Builder, aligned: bool| -> Offset<()> {
            let mut table_writer = TableWriter::<6>::default();
            if aligned {
                table_writer.write_entry::<u64>(0);
            } else {
                table_writer.write_entry::<Unaligned8>(0);
            }
            unsafe {
                table_writer.finish_with_offset(builder, |object_writer| {
                    if aligned {
                        object_writer.write::<u64, _, 8>(&7u64);
                    } else {
                        object_writer.write::<Unaligned8, _, 8>(&UnalignedU64(7));
                    }
                })
            }
        };
        let unaligned = write(&mut builder, false);
        assert_eq!(unaligned, write(&mut builder, false));
        let aligned = write(&mut builder, true);
        assert_ne!(unaligned, aligned);
        assert_eq!(aligned, write(&mut builder, true));
    }

    #[test]
    fn clear_empties_the_cache() {
        let mut builder = cached_builder();
        table(&mut builder, 1, None, 1.0);
        builder.clear();
        assert!(builder.table_cache_enabled());
        table(&mut builder, 1, None, 1.0);
        let len = builder.len();
        assert!(len > 0);
        table(&mut builder, 1, None, 1.0);
        assert_eq!(builder.len(), len);
    }

    #[test]
    fn disabling_drops_the_cache() {
        let mut builder = cached_builder();
        let first = table(&mut builder, 1, None, 1.0);
        builder.set_table_cache(false);
        builder.set_table_cache(true);
        assert_ne!(first, table(&mut builder, 1, None, 1.0));
    }

    #[test]
    fn vectors_of_offsets_share_an_offset() {
        let mut builder = cached_builder();
        let a = table(&mut builder, 1, None, 0.0);
        let b = table(&mut builder, 2, None, 0.0);
        let ab = builder.create_vector([a, b].as_slice());
        let len = builder.len();
        assert_eq!(ab, builder.create_vector([a, b].as_slice()));
        assert_eq!(ab, builder.create_vector([a, b]));
        assert_eq!(ab, builder.create_vector(alloc::vec![a, b]));
        assert_eq!(builder.len(), len);
        assert_ne!(ab, builder.create_vector([b, a].as_slice()));
        assert_ne!(ab, builder.create_vector([a].as_slice()));
        let none: &[Offset<()>] = &[];
        let empty = builder.create_vector(none);
        assert_eq!(empty, builder.create_vector(none));
    }

    #[test]
    fn vectors_of_scalars_are_left_alone() {
        let mut builder = cached_builder();
        let first = builder.create_vector([1u32, 2].as_slice());
        assert_ne!(first, builder.create_vector([1u32, 2].as_slice()));
    }
}
