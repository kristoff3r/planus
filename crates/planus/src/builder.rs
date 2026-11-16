use core::{marker::PhantomData, mem::MaybeUninit};

#[cfg(feature = "vector-cache")]
use crate::VectorWrite;
use crate::{
    Offset, Primitive, UnionVectorOffset, WriteAsOffset, WriteAsUnionVector, backvec::BackVec,
};

#[derive(Debug)]
/// Builder for serializing flatbuffers.
///
///
/// # Examples
/// ```
/// use planus::Builder;
/// use planus_example::monster_generated::my_game::sample::Weapon;
/// let mut builder = Builder::new();
/// let weapon = Weapon::create(&mut builder, "Axe", 24);
/// builder.finish(weapon, None);
/// ```
pub struct Builder {
    pub(crate) inner: BackVec,

    #[cfg(feature = "vtable-cache")]
    vtable_cache: crate::builder_cache::Cache<crate::builder_cache::VTable>,

    // byte slices and strings are sufficiently similar that they can share a
    // cache type, but they cannot share the cache because strings need to be
    // null byte terminated
    #[cfg(feature = "string-cache")]
    pub(crate) string_cache: crate::builder_cache::Cache<crate::builder_cache::ByteVec>,
    #[cfg(feature = "bytes-cache")]
    pub(crate) bytes_cache: crate::builder_cache::Cache<crate::builder_cache::ByteVec>,
    #[cfg(feature = "vector-cache")]
    vector_cache: crate::builder_cache::VectorCache,
    // Holds the serialized form of a vector while it is looked up in
    // `vector_cache`, kept around to reuse its allocation
    #[cfg(feature = "vector-cache")]
    vector_cache_scratch: alloc::vec::Vec<u8>,

    // This is a bit complicated. The buffer has support for guaranteeing a
    // specific write gets a specific alignment. It has many writes and thus
    // many promises, so how does keep track of this this across those promises, even
    // when writing from the back?
    //
    // The algorithm works by aggregating all of the promises into one big promise.
    // Specifically, we promise that the remaining part of the buffer will always
    // be of size `self.delayed_bytes + self.alignment() * K` where we are free to
    // choose K as we want.
    //
    // Initially we set `delayed_bytes` to 0 and `alignment` to 1, i.e. we have
    // only promised to write `0 + 1 * K` bytes, for any `K` we choose, which will
    // be true no matter how many bytes we write.
    //
    // Whenever we get a new request `(req_size, req_alignment)`, then that
    // `req_size` will be counted towards the previous promises, i.e. we need
    // to decrease `self.delayed_bytes()` by `req_bytes` and calculate the new value
    // of `req_size` modulo `alignment`. However we also need to fulfil this new
    // promise.
    //
    // To do this, we do two things. 1) We insert sufficient padding, before the
    // current request, to make sure that the current request is compatible with
    // the previous ones. 2) We set `alignment = alignment.max(req_alignment)`.
    //
    // One small wrinkle is that we do not store `alignment` directly for performance
    // reasons. Instead we store `alignment_mask = alignment - 1`, so we can do use
    // binary and (`&`) instead of modulo (`%`).
    delayed_bytes: usize,
    alignment_mask: usize,

    #[cfg(debug_assertions)]
    // Bytes missing to be written by a call to prepare_write
    missing_bytes: usize,
}

impl Default for Builder {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl Builder {
    /// Creates a new Builder.
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    /// Gets the length of the internal buffer in bytes.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the internal buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Creates a new builder with a specific internal capacity already allocated.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: BackVec::with_capacity(capacity),

            delayed_bytes: 0,
            alignment_mask: 0,

            #[cfg(feature = "vtable-cache")]
            vtable_cache: crate::builder_cache::Cache::default(),

            #[cfg(feature = "string-cache")]
            string_cache: crate::builder_cache::Cache::default(),

            #[cfg(feature = "bytes-cache")]
            bytes_cache: crate::builder_cache::Cache::default(),

            #[cfg(feature = "vector-cache")]
            vector_cache: crate::builder_cache::VectorCache::default(),
            #[cfg(feature = "vector-cache")]
            vector_cache_scratch: alloc::vec::Vec::new(),

            #[cfg(debug_assertions)]
            missing_bytes: 0,
        }
    }

    /// Serializes a string and returns the offset to it
    pub fn create_string(&mut self, v: impl WriteAsOffset<str>) -> Offset<str> {
        v.prepare(self)
    }

    /// Serializes a slice and returns the offset to it
    pub fn create_vector<T>(&mut self, v: impl WriteAsOffset<[T]>) -> Offset<[T]> {
        v.prepare(self)
    }

    /// Serializes a slice of union values and returns the offset to it
    pub fn create_union_vector<T>(
        &mut self,
        v: impl WriteAsUnionVector<T>,
    ) -> UnionVectorOffset<T> {
        v.prepare(self)
    }

    /// Resets the builders internal state and clears the internal buffer.
    pub fn clear(&mut self) {
        self.inner.clear();
        #[cfg(feature = "vtable-cache")]
        self.vtable_cache.clear();
        #[cfg(feature = "string-cache")]
        self.string_cache.clear();
        #[cfg(feature = "bytes-cache")]
        self.bytes_cache.clear();
        #[cfg(feature = "vector-cache")]
        self.vector_cache.clear();
        self.delayed_bytes = 0;
        self.alignment_mask = 0;
        #[cfg(debug_assertions)]
        {
            self.missing_bytes = 0;
        }
    }

    pub(crate) fn prepare_write(&mut self, size: usize, alignment_mask: usize) -> usize {
        debug_assert!((alignment_mask + 1) & alignment_mask == 0); // Check that the alignment is a power of two
        #[cfg(debug_assertions)]
        debug_assert_eq!(self.missing_bytes, 0);

        let delayed_bytes = self.delayed_bytes.wrapping_sub(size) & self.alignment_mask;
        let needed_padding = delayed_bytes & alignment_mask;
        self.delayed_bytes = delayed_bytes.wrapping_sub(needed_padding);
        self.alignment_mask |= alignment_mask;
        self.inner.reserve(size.wrapping_add(needed_padding));
        // TODO: investigate if it makes sense to use an extend_with_zeros_unchecked for performance, given
        // that we know we have enough space
        self.inner.extend_with_zeros(needed_padding);

        debug_assert_eq!(self.delayed_bytes & alignment_mask, 0);

        #[cfg(debug_assertions)]
        {
            self.missing_bytes = size;
        }

        self.len() + size
    }

    #[doc(hidden)]
    pub fn current_offset<T: ?Sized>(&self) -> Offset<T> {
        Offset {
            offset: self.inner.len() as u32,
            phantom: PhantomData,
        }
    }

    pub(crate) fn write_vtable(&mut self, vtable: &[u8]) -> usize {
        const VTABLE_ALIGNMENT: usize = 2;
        const VTABLE_ALIGNMENT_MASK: usize = VTABLE_ALIGNMENT - 1;

        #[cfg(feature = "vtable-cache")]
        let hash = {
            let hash = self.vtable_cache.hash(vtable);
            if let Some(offset) = self.vtable_cache.get(self.inner.as_slice(), hash, vtable) {
                return offset.into();
            }
            hash
        };

        let offset = self.prepare_write(vtable.len(), VTABLE_ALIGNMENT_MASK);
        self.write(vtable);
        #[cfg(feature = "vtable-cache")]
        self.vtable_cache
            .insert(hash, offset.try_into().unwrap(), self.inner.as_slice());
        offset
    }

    /// Writes a vector whose serialized bytes do not depend on its position,
    /// or returns the offset of an identical vector written earlier.
    #[cfg(feature = "vector-cache")]
    pub(crate) fn write_cached_vector<T, P>(&mut self, values: &[T::Value]) -> Offset<[P]>
    where
        T: VectorWrite<P>,
        P: Primitive,
    {
        let data_len = T::STRIDE.checked_mul(values.len()).unwrap();
        let alignment_mask = P::ALIGNMENT_MASK.max(u32::ALIGNMENT_MASK);

        let mut key = core::mem::take(&mut self.vector_cache_scratch);
        key.clear();
        key.reserve(4 + data_len);
        key.extend_from_slice(&u32::try_from(values.len()).unwrap().to_le_bytes());
        // SAFETY: `write_values` initializes `data_len` bytes. The bytes do not
        // depend on the buffer position, so it only has to keep the position
        // arithmetic of the elements from underflowing, which the position of
        // the data in a real buffer would also do.
        unsafe {
            T::write_values(
                values,
                key.spare_capacity_mut().as_mut_ptr(),
                u32::try_from(data_len).unwrap(),
            );
            key.set_len(4 + data_len);
        }

        let hash = self.vector_cache.hash(&key);
        let offset = if let Some(offset) =
            self.vector_cache
                .get(self.inner.as_slice(), hash, &key, alignment_mask)
        {
            offset.into()
        } else {
            self.prepare_write(data_len, alignment_mask);
            self.write(&key[4..]);
            self.prepare_write(4, 0);
            self.write(&key[..4]);
            let offset = self.current_offset();
            self.vector_cache.insert(
                hash,
                offset.into(),
                data_len,
                alignment_mask,
                self.inner.as_slice(),
            );
            offset
        };
        self.vector_cache_scratch = key;
        offset
    }

    pub(crate) fn write(&mut self, buffer: &[u8]) {
        #[cfg(debug_assertions)]
        {
            self.missing_bytes = self.missing_bytes.checked_sub(buffer.len()).unwrap();
        }
        // TODO: investigate if it makes sense to use an extend_from_slice_unchecked for performance, given
        // that we know we have enough space
        self.inner.extend_from_slice(buffer);
    }

    #[doc(hidden)]
    /// # Safety
    ///
    /// If you call this function, then the inner close needs to *always* initialize all the bytes in the slice
    pub unsafe fn write_with(
        &mut self,
        size: usize,
        alignment_mask: usize,
        f: impl FnOnce(u32, &mut [MaybeUninit<u8>]),
    ) {
        unsafe {
            let offset = self.prepare_write(size, alignment_mask) as u32;
            self.inner.extend_write(size, |bytes| f(offset, bytes));
            #[cfg(debug_assertions)]
            {
                self.missing_bytes = self.missing_bytes.checked_sub(size).unwrap();
            }
        }
    }

    /// Finish writing the internal buffer and return a byte slice of it.
    ///
    /// This will make sure all alignment requirements are fullfilled and that
    /// the file identifier has been written if specified.
    ///
    /// # Examples
    /// ```
    /// use planus::Builder;
    /// use planus_example::monster_generated::my_game::sample::Weapon;
    /// let mut builder = Builder::new();
    /// let weapon = Weapon::create(&mut builder, "Axe", 24);
    /// builder.finish(weapon, None);
    /// ```
    ///
    /// It can also be used to directly serialize an owned flatbuffers struct
    /// ```
    /// use planus::Builder;
    /// use planus_example::monster_generated::my_game::sample::Weapon;
    /// let mut builder = Builder::new();
    /// let weapon = Weapon {
    ///     name: Some("Sword".to_string()),
    ///     damage: 12,
    /// };
    /// let data = builder.finish(&weapon, None);
    /// ```
    pub fn finish<T>(
        &mut self,
        root: impl WriteAsOffset<T>,
        file_identifier: Option<[u8; 4]>,
    ) -> &[u8] {
        let root = root.prepare(self);

        if let Some(file_identifier) = file_identifier {
            // TODO: how does alignment interact with file identifiers? Is the alignment with out without the header?
            let offset = self.prepare_write(
                8,
                <Offset<T> as Primitive>::ALIGNMENT_MASK.max(self.alignment_mask),
            ) as u32;
            self.write(&(offset - 4 - root.offset).to_le_bytes());
            self.write(&file_identifier);
        } else {
            let offset = self.prepare_write(
                4,
                <Offset<T> as Primitive>::ALIGNMENT_MASK.max(self.alignment_mask),
            ) as u32;
            self.write(&(offset - root.offset).to_le_bytes());
        }
        debug_assert_eq!(self.delayed_bytes, 0);
        self.as_slice()
    }

    /// Returns a reference to the current data buffer.
    ///
    /// It will return the same slice as the one return by [`finish`], unless additional data has been appened afterwards.
    ///
    /// [`finish`]: Self::finish
    pub fn as_slice(&self) -> &[u8] {
        self.inner.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use rand::RngExt;

    use super::*;

    #[test]
    fn test_buffer_random() {
        let mut slice = [0; 128];
        let mut rng = rand::rng();
        let mut back_offsets: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();

        for _ in 0..50 {
            let mut builder = Builder::new();
            back_offsets.clear();

            for byte in 1..50 {
                let size: usize = rng.random::<u32>() as usize % slice.len();
                let slice = &mut slice[..size];
                for p in &mut *slice {
                    *p = byte;
                }
                let alignment: usize = 1 << (rng.random::<u32>() % 5);
                let alignment_mask = alignment - 1;
                let offset = builder.prepare_write(size, alignment_mask);
                let len_before = builder.inner.len();
                builder.write(slice);
                assert_eq!(offset, builder.len());
                assert!(builder.inner.len() < len_before + slice.len() + alignment);
                back_offsets.push((builder.inner.len(), size, alignment));
            }
            let random_padding: usize = rng.random::<u32>() as usize % slice.len();
            let slice = &mut slice[..random_padding];
            for p in &mut *slice {
                *p = rng.random();
            }
            builder.prepare_write(random_padding, 1);
            builder.write(slice);
            let buffer = builder.finish(builder.current_offset::<()>(), None);

            for (i, (back_offset, size, alignment)) in back_offsets.iter().enumerate() {
                let byte = (i + 1) as u8;
                let offset = buffer.len() - back_offset;
                assert_eq!(offset % alignment, 0);
                assert!(buffer[offset..offset + size].iter().all(|&b| b == byte));
            }
        }
    }

    #[test]
    fn test_buffer_align() {
        let mut builder = Builder::new();
        builder.prepare_write(3, 0);
        builder.write(b"MNO");
        assert_eq!(builder.delayed_bytes, 0);
        builder.prepare_write(4, 1);
        builder.write(b"IJKL");
        assert_eq!(builder.delayed_bytes, 0);
        builder.prepare_write(8, 3);
        builder.write(b"ABCDEFGH");
        assert_eq!(builder.delayed_bytes, 0);
        builder.prepare_write(7, 0);
        builder.write(b"0123456");
        assert_eq!(
            builder.finish(builder.current_offset::<()>(), None),
            b"\x05\x00\x00\x00\x000123456ABCDEFGHIJKLMNO"
        );

        builder.clear();
        builder.prepare_write(4, 3);
        builder.write(b"IJKL");
        assert_eq!(builder.delayed_bytes, 0);
        builder.prepare_write(1, 0);
        builder.write(b"X");
        assert_eq!(builder.delayed_bytes, 3);
        builder.prepare_write(1, 0);
        builder.write(b"Y");
        assert_eq!(builder.delayed_bytes, 2);
        builder.prepare_write(8, 7);
        builder.write(b"ABCDEFGH");
        assert_eq!(builder.delayed_bytes, 0);
        assert_eq!(
            builder.finish(builder.current_offset::<()>(), None),
            b"\x08\x00\x00\x00\x00\x00\x00\x00ABCDEFGH\x00\x00YXIJKL"
        );
    }

    #[cfg(feature = "vector-cache")]
    mod vector_cache {
        use super::*;

        /// Bytes of the vector at `offset` in a finished buffer, including the
        /// length prefix, and the position of its element data
        fn vector_bytes<T: ?Sized>(
            buffer: &[u8],
            offset: Offset<T>,
            stride: usize,
        ) -> (&[u8], usize) {
            let start = buffer.len() - offset.offset as usize;
            let len = u32::from_le_bytes(buffer[start..start + 4].try_into().unwrap()) as usize;
            (&buffer[start..start + 4 + len * stride], start + 4)
        }

        #[test]
        fn shares_identical_vectors() {
            let mut builder = Builder::new();
            let a = builder.create_vector([1u32, 2, 3].as_slice());
            let b = builder.create_vector(alloc::vec![1u32, 2, 3]);
            let c = builder.create_vector([1u32, 2, 3]);
            let d = builder.create_vector([1u32, 2, 4].as_slice());
            let e = builder.create_vector([1i32, 2, 3].as_slice());
            assert_eq!(a, b);
            assert_eq!(a, c);
            assert_ne!(a, d);
            // Elements of another type with the same bytes and alignment
            assert_eq!(a.offset, e.offset);

            let f = builder.create_vector([true, false].as_slice());
            let g = builder.create_vector([true, false].as_slice());
            assert_eq!(f, g);

            let buffer = builder.finish(builder.current_offset::<()>(), None);
            assert_eq!(
                vector_bytes(buffer, a, 4).0,
                b"\x03\x00\x00\x00\x01\x00\x00\x00\x02\x00\x00\x00\x03\x00\x00\x00"
            );
            assert_eq!(vector_bytes(buffer, d, 4).0[12..], [4, 0, 0, 0]);
        }

        #[test]
        fn respects_alignment() {
            let mut builder = Builder::new();
            let empty_u32 = builder.create_vector(alloc::vec::Vec::<u32>::new());
            // The empty `[u32]` was written with 4-byte alignment only
            let empty_u64 = builder.create_vector(alloc::vec::Vec::<u64>::new());
            assert_ne!(empty_u32.offset, empty_u64.offset);
            let empty_u16 = builder.create_vector(alloc::vec::Vec::<u16>::new());
            assert!(empty_u16.offset == empty_u32.offset || empty_u16.offset == empty_u64.offset);
            let empty_u64_again = builder.create_vector(alloc::vec::Vec::<u64>::new());
            assert_eq!(empty_u64, empty_u64_again);

            let u64s = builder.create_vector([7u64].as_slice());
            builder.prepare_write(1, 0);
            builder.write(&[0]);
            let u64s_again = builder.create_vector([7u64].as_slice());
            assert_eq!(u64s, u64s_again);

            let buffer = builder.finish(builder.current_offset::<()>(), None);
            assert_eq!(vector_bytes(buffer, empty_u64, 8).1 % 8, 0);
            assert_eq!(vector_bytes(buffer, u64s, 8).1 % 8, 0);
        }

        #[test]
        fn shares_empty_offset_vectors() {
            let mut builder = Builder::new();
            let empty_u32 = builder.create_vector(alloc::vec::Vec::<u32>::new());
            let empty_strings = builder.create_vector(alloc::vec::Vec::<Offset<str>>::new());
            assert_eq!(empty_u32.offset, empty_strings.offset);
        }

        #[test]
        fn never_shares_offset_vectors() {
            let mut builder = Builder::new();
            let a = builder.create_string("a");
            let strings_a = builder.create_vector([a].as_slice());
            let b = builder.create_string("b");
            let strings_b = builder.create_vector([b].as_slice());
            assert_ne!(strings_a.offset, strings_b.offset);

            // Both vectors hold the same relative offset, so sharing them by
            // their bytes would point the second one at "a"
            let buffer = builder.finish(builder.current_offset::<()>(), None);
            assert_eq!(
                vector_bytes(buffer, strings_a, 4).0,
                vector_bytes(buffer, strings_b, 4).0
            );
        }

        #[test]
        fn clear_resets_cache() {
            let mut builder = Builder::new();
            builder.create_vector([1u16, 2].as_slice());
            builder.clear();
            let a = builder.create_vector([3u16, 4, 5, 6, 7, 8].as_slice());
            let b = builder.create_vector([1u16, 2].as_slice());
            assert_ne!(a, b);
            let buffer = builder.finish(builder.current_offset::<()>(), None);
            assert_eq!(
                vector_bytes(buffer, b, 2).0,
                b"\x02\x00\x00\x00\x01\x00\x02\x00"
            );
        }
    }
}
