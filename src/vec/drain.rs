use core::{
    fmt,
    iter::FusedIterator,
    mem::{self, size_of},
    ptr::{self, NonNull},
    slice,
};

use super::VecView;

/// A draining iterator for [`Vec`](super::Vec).
///
/// This `struct` is created by [`Vec::drain`](super::Vec::drain).
/// See its documentation for more.
///
/// # Example
///
/// ```
/// use heapless::{vec, Vec};
///
/// let mut v = Vec::<_, 4>::from_array([0, 1, 2]);
/// let iter: vec::Drain<'_, _> = v.drain(..);
/// ```
pub struct Drain<'a, T: 'a> {
    /// Index of tail to preserve
    pub(super) tail_start: usize,
    /// Length of tail
    pub(super) tail_len: usize,
    /// Current remaining range to remove
    pub(super) iter: slice::Iter<'a, T>,
    pub(super) vec: NonNull<VecView<T>>,
}

impl<T: fmt::Debug> fmt::Debug for Drain<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Drain").field(&self.iter.as_slice()).finish()
    }
}

impl<T> Drain<'_, T> {
    /// Returns the remaining items of this iterator as a slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use heapless::{vec, Vec};
    ///
    /// let mut vec = Vec::<_, 3>::from_array(['a', 'b', 'c']);
    /// let mut drain = vec.drain(..);
    /// assert_eq!(drain.as_slice(), &['a', 'b', 'c']);
    /// let _ = drain.next().unwrap();
    /// assert_eq!(drain.as_slice(), &['b', 'c']);
    /// ```
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        self.iter.as_slice()
    }
}

impl<T> AsRef<[T]> for Drain<'_, T> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

unsafe impl<T: Sync> Sync for Drain<'_, T> {}
unsafe impl<T: Send> Send for Drain<'_, T> {}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        self.iter
            .next()
            .map(|elt| unsafe { ptr::read(elt as *const _) })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for Drain<'_, T> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        self.iter
            .next_back()
            .map(|elt| unsafe { ptr::read(elt as *const _) })
    }
}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        /// Moves back the un-`Drain`ed elements to restore the original `Vec`.
        struct DropGuard<'r, 'a, T>(&'r mut Drain<'a, T>);

        impl<T> Drop for DropGuard<'_, '_, T> {
            fn drop(&mut self) {
                if self.0.tail_len > 0 {
                    unsafe {
                        let source_vec = self.0.vec.as_mut();
                        // memmove back untouched tail, update to new length
                        let start = source_vec.len();
                        let tail = self.0.tail_start;
                        if tail != start {
                            let dst = source_vec.as_mut_ptr().add(start);
                            let src = source_vec.as_ptr().add(tail);
                            ptr::copy(src, dst, self.0.tail_len);
                        }
                        source_vec.set_len(start + self.0.tail_len);
                    }
                }
            }
        }

        let iter = mem::take(&mut self.iter);
        let drop_len = iter.len();

        let mut vec = self.vec;

        if size_of::<T>() == 0 {
            // ZSTs have no identity, so we don't need to move them around, we only need to drop the correct amount.
            // this can be achieved by manipulating the `Vec` length instead of moving values out from `iter`.
            unsafe {
                let vec = vec.as_mut();
                let old_len = vec.len();
                vec.set_len(old_len + drop_len + self.tail_len);
                vec.truncate(old_len + self.tail_len);
            }

            return;
        }

        // ensure elements are moved back into their appropriate places, even when drop_in_place panics
        let _guard = DropGuard(self);

        if drop_len == 0 {
            return;
        }

        // as_slice() must only be called when iter.len() is > 0 because
        // it also gets touched by vec::Splice which may turn it into a dangling pointer
        // which would make it and the vec pointer point to different allocations which would
        // lead to invalid pointer arithmetic below.
        let drop_ptr = iter.as_slice().as_ptr();

        unsafe {
            // drop_ptr comes from a slice::Iter which only gives us a &[T] but for drop_in_place
            // a pointer with mutable provenance is necessary. Therefore we must reconstruct
            // it from the original vec but also avoid creating a &mut to the front since that could
            // invalidate raw pointers to it which some unsafe code might rely on.
            let vec_ptr = vec.as_mut().as_mut_ptr();
            // FIXME: Replace with `sub_ptr` once stable.
            let drop_offset = (drop_ptr as usize - vec_ptr as usize) / size_of::<T>();
            let to_drop = ptr::slice_from_raw_parts_mut(vec_ptr.add(drop_offset), drop_len);
            ptr::drop_in_place(to_drop);
        }
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {}

impl<T> FusedIterator for Drain<'_, T> {}

#[cfg(test)]
mod tests {
    use super::super::Vec;

    #[test]
    fn drain_front() {
        let mut vec = Vec::<_, 8>::from_array([1, 2, 3, 4]);
        let mut it = vec.drain(..1);
        assert_eq!(it.next(), Some(1));
        drop(it);
        assert_eq!(vec, &[2, 3, 4]);
    }

    #[test]
    fn drain_middle() {
        let mut vec = Vec::<_, 8>::from_array([1, 2, 3, 4]);
        let mut it = vec.drain(1..3);
        assert_eq!(it.next(), Some(2));
        assert_eq!(it.next(), Some(3));
        drop(it);
        assert_eq!(vec, &[1, 4]);
    }

    #[test]
    fn drain_end() {
        let mut vec = Vec::<_, 8>::from_array([1, 2, 3, 4]);
        let mut it = vec.drain(3..);
        assert_eq!(it.next(), Some(4));
        drop(it);
        assert_eq!(vec, &[1, 2, 3]);
    }

    #[test]
    #[cfg(not(feature="copy"))]
    fn drain_drop_rest() {
        droppable!();

        let mut vec = Vec::<_, 8>::from_array([
            Droppable::new(),
            Droppable::new(),
            Droppable::new(),
            Droppable::new(),
        ]);
        assert_eq!(Droppable::count(), 4);

        let mut iter = vec.drain(2..);
        assert_eq!(iter.next().unwrap().0, 3);
        drop(iter);
        assert_eq!(Droppable::count(), 2);

        assert_eq!(vec.len(), 2);
        assert_eq!(vec.remove(0).0, 1);
        assert_eq!(Droppable::count(), 1);

        drop(vec);
        assert_eq!(Droppable::count(), 0);
    }
}
