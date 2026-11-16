use core::mem::{self, MaybeUninit};

#[cfg(feature = "table-cache")]
use crate::table_cache::{KEY_POSITION, KeyKind};
use crate::{Builder, Offset, Primitive, WriteAsPrimitive};

#[doc(hidden)]
pub struct TableWriter<const VTABLE_MAX_BYTES: usize> {
    vtable_buffer: [u8; VTABLE_MAX_BYTES],
    vtable_size: usize,
    object_size: usize,
    object_alignment_mask: usize,
}

impl<const VTABLE_MAX_BYTES: usize> Default for TableWriter<VTABLE_MAX_BYTES> {
    fn default() -> Self {
        Self {
            vtable_buffer: [0; VTABLE_MAX_BYTES], // including vtable size and object size
            vtable_size: 4, // 4 bytes always needed for vtable and object sizes
            object_size: 4, // 4 bytes always needed for vtable offset
            object_alignment_mask: i32::ALIGNMENT_MASK, // objects always contain a vtable offset, which is an i32 aligned
        }
    }
}

fn write_array<const N: usize, const M: usize>(buf: &mut [u8; N], offset: usize, data: [u8; M]) {
    buf[offset..offset + M].copy_from_slice(&data)
}

impl<const VTABLE_MAX_BYTES: usize> TableWriter<VTABLE_MAX_BYTES> {
    /// # Safety
    ///
    /// Must be called in alignment order with the most-aligned object first
    #[inline]
    pub fn write_entry<P: Primitive>(&mut self, vtable_index: usize) {
        self.object_alignment_mask = self.object_alignment_mask.max(P::ALIGNMENT_MASK);

        let offset = 2 * (vtable_index + 2); // 2 bytes per index, skip the vtable size and object size

        write_array(
            &mut self.vtable_buffer,
            offset,
            (self.object_size as u16).to_le_bytes(),
        );

        self.object_size += P::SIZE;
        self.vtable_size = self.vtable_size.max(offset + 2);
        debug_assert!(self.vtable_size <= self.vtable_buffer.len());
    }

    /// Writes the table and its vtable.
    ///
    /// # Safety
    ///
    /// `f` must write every entry announced with [`Self::write_entry`], in the
    /// same order.
    #[inline]
    pub unsafe fn finish(mut self, builder: &mut Builder, f: impl FnOnce(&mut ObjectWriter<'_>)) {
        self.finish_vtable();
        unsafe { self.write(builder, f) }
    }

    /// Writes the table and its vtable and returns the offset of the table.
    ///
    /// When the builder deduplicates tables (see `Builder::set_table_cache`)
    /// and an equal table has been written before, the earlier table's offset
    /// is returned and nothing is written. `f` may be called twice.
    ///
    /// # Safety
    ///
    /// `f` must write every entry announced with [`Self::write_entry`], in the
    /// same order, and write the same values on every call.
    #[inline]
    pub unsafe fn finish_with_offset<T: ?Sized>(
        mut self,
        builder: &mut Builder,
        f: impl Fn(&mut ObjectWriter<'_>),
    ) -> Offset<T> {
        self.finish_vtable();

        #[cfg(feature = "table-cache")]
        if let Some(cache) = &mut builder.table_cache {
            cache.start_key(KeyKind::Table, self.object_alignment_mask);
            cache.extend_key(&self.vtable_buffer[..self.vtable_size]);
            f(&mut ObjectWriter {
                offset: KEY_POSITION,
                bytes: cache.reserve_key(self.object_size - 4),
            });
            return match cache.get() {
                Ok(offset) => Offset {
                    offset,
                    phantom: core::marker::PhantomData,
                },
                Err(hash) => {
                    unsafe { self.write(builder, f) };
                    let offset = builder.current_offset();
                    if let Some(cache) = &mut builder.table_cache {
                        cache.insert(hash, offset.offset);
                    }
                    offset
                }
            };
        }

        unsafe { self.write(builder, f) };
        builder.current_offset()
    }

    #[inline]
    fn finish_vtable(&mut self) {
        write_array(
            &mut self.vtable_buffer,
            0,
            (self.vtable_size as u16).to_le_bytes(),
        );
        write_array(
            &mut self.vtable_buffer,
            2,
            (self.object_size as u16).to_le_bytes(),
        );
    }

    #[inline]
    unsafe fn write(&self, builder: &mut Builder, f: impl FnOnce(&mut ObjectWriter<'_>)) {
        unsafe {
            let vtable_offset = builder.write_vtable(&self.vtable_buffer[..self.vtable_size]);

            builder.write_with(
                self.object_size - 4,
                self.object_alignment_mask,
                |offset, bytes| {
                    f(&mut ObjectWriter { offset, bytes });
                },
            );
            builder.write_with(4, 0, |buffer_position, bytes| {
                let len = (vtable_offset as i32 - buffer_position as i32)
                    .to_le_bytes()
                    .map(MaybeUninit::new);
                bytes.copy_from_slice(&len);
            });
        }
    }
}

#[doc(hidden)]
pub struct ObjectWriter<'a> {
    offset: u32,
    bytes: &'a mut [MaybeUninit<u8>],
}

impl ObjectWriter<'_> {
    /// # Safety
    ///
    /// Must be called in alignment order with the most-aligned object first
    #[inline(always)]
    pub unsafe fn write<P: Primitive, T: WriteAsPrimitive<P>, const SIZE: usize>(
        &mut self,
        value: &T,
    ) {
        assert_eq!(P::SIZE, SIZE);
        let (cur, remaining) = mem::take(&mut self.bytes).split_at_mut(P::SIZE);
        self.bytes = remaining;
        value.write(
            array_init_cursor::Cursor::<'_, u8, SIZE>::new(cur.try_into().unwrap()),
            self.offset,
        );
        self.offset -= P::SIZE as u32;
    }
}
