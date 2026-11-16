use core::mem::MaybeUninit;

use crate::{Offset, UnionVectorOffset, builder::Builder, traits::*};

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
        // The element data and the length prefix are written separately so that
        // the alignment applies to the elements, which may need more than the
        // 4 bytes of the prefix.
        // SAFETY: The inner closure always initializes the entire buffer, because it calls `write_values` with `N` values each of length `T::STRIDE`.
        unsafe {
            builder.write_with(
                T::STRIDE.checked_mul(N).unwrap(),
                P::ALIGNMENT_MASK.max(u32::ALIGNMENT_MASK),
                |buffer_position, bytes| {
                    T::write_values(&tmp, bytes.as_mut_ptr(), buffer_position);
                },
            );
        }

        // SAFETY: The inner closure always initializes the entire buffer, because it calls `copy_from_slice` with an array of len 4
        unsafe {
            builder.write_with(4, 0, |_buffer_position, bytes| {
                let len = (N as u32).to_le_bytes().map(MaybeUninit::new);
                bytes.copy_from_slice(&len);
            });
        }
        builder.current_offset()
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
