use core::mem::MaybeUninit;

use crate::{Cursor, Offset, UnionVectorOffset, builder::Builder, traits::*};

impl<T, P, const N: usize> WriteAsOffset<[P]> for [T; N]
where
    P: Primitive,
    T: VectorWrite<P>,
{
    fn prepare(&self, builder: &mut Builder) -> Offset<[P]> {
        let mut tmp: [MaybeUninit<T::Value>; N] = unsafe { MaybeUninit::uninit().assume_init() };
        for (t, v) in tmp.iter_mut().zip(self.iter()) {
            t.write(v.prepare(builder));
        }
        // TODO: replace with std::mem::MaybeUninit::array_assume_init when it becomes stable
        //       https://github.com/rust-lang/rust/issues/80908
        let tmp =
            unsafe { (&tmp as *const [MaybeUninit<T::Value>; N] as *const [T::Value; N]).read() };
        let alignment_mask = P::ALIGNMENT_MASK.max(3);
        let write = |builder: &mut Builder| {
            unsafe {
                builder.write_with(
                    4 + T::STRIDE.checked_mul(self.len()).unwrap(),
                    alignment_mask,
                    |buffer_position, bytes| {
                        let bytes = bytes.as_mut_ptr();

                        (self.len() as u32).write(
                            Cursor::new(&mut *(bytes as *mut [MaybeUninit<u8>; 4])),
                            buffer_position,
                        );

                        T::write_values(&tmp, bytes.add(4), buffer_position - 4);
                    },
                )
            };
            builder.current_offset()
        };

        #[cfg(feature = "table-cache")]
        return crate::table_cache::dedup_vector::<T, P>(builder, &tmp, alignment_mask, write);
        #[cfg(not(feature = "table-cache"))]
        write(builder)
    }
}

impl<T, P, const N: usize> WriteAs<Offset<[P]>> for [T; N]
where
    P: Primitive,
    T: VectorWrite<P>,
{
    type Prepared = Offset<[P]>;

    fn prepare(&self, builder: &mut Builder) -> Offset<[P]> {
        WriteAsOffset::prepare(self, builder)
    }
}

impl<T, P, const N: usize> WriteAsOptional<Offset<[P]>> for [T; N]
where
    P: Primitive,
    T: VectorWrite<P>,
{
    type Prepared = Offset<[P]>;

    #[inline]
    fn prepare(&self, builder: &mut Builder) -> Option<Offset<[P]>> {
        Some(WriteAsOffset::prepare(self, builder))
    }
}

impl<T, P, const N: usize> WriteAsUnionVector<P> for [T; N]
where
    T: WriteAsUnion<P>,
{
    fn prepare(&self, builder: &mut Builder) -> UnionVectorOffset<P> {
        WriteAsUnionVector::prepare(self.as_slice(), builder)
    }
}

impl<T, P, const N: usize> WriteAsDefaultUnionVector<P> for [T; N]
where
    T: WriteAsUnion<P>,
{
    fn prepare(&self, builder: &mut Builder) -> Option<UnionVectorOffset<P>> {
        if self.is_empty() {
            None
        } else {
            Some(WriteAsUnionVector::prepare(self.as_slice(), builder))
        }
    }
}

impl<T, P, const N: usize> WriteAsOptionalUnionVector<P> for [T; N]
where
    T: WriteAsUnion<P>,
{
    #[inline]
    fn prepare(&self, builder: &mut Builder) -> Option<UnionVectorOffset<P>> {
        Some(WriteAsUnionVector::prepare(self.as_slice(), builder))
    }
}
