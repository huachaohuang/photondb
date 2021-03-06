use std::{
    cmp::Ordering,
    marker::PhantomData,
    mem::size_of,
    ops::{Deref, DerefMut},
    slice,
};

use super::*;

/// A builder to create data pages.
pub struct DataPageBuilder {
    base: PageBuilder,
    offsets_size: usize,
    payload_size: usize,
}

impl Default for DataPageBuilder {
    fn default() -> Self {
        Self {
            base: PageBuilder::new(PageKind::Data),
            offsets_size: 0,
            payload_size: 0,
        }
    }
}

// TODO: Optimizes the page layout with
// https://cseweb.ucsd.edu//~csjgwang/pubs/ICDE17_BwTree.pdf
impl DataPageBuilder {
    fn add<K, V>(&mut self, key: &K, value: &V)
    where
        K: Encodable,
        V: Encodable,
    {
        self.offsets_size += size_of::<u32>();
        self.payload_size += key.encode_size() + value.encode_size();
    }

    fn size(&self) -> usize {
        self.offsets_size + self.payload_size
    }

    /// Builds an empty data page.
    pub fn build<A>(self, alloc: &A) -> Result<DataPageBuf, A::Error>
    where
        A: PageAlloc,
    {
        let ptr = self.base.build(alloc, self.size());
        ptr.map(|ptr| unsafe { DataPageBuf::new(ptr, self) })
    }

    /// Builds a data page with entries from the given iterator.
    pub fn build_from_iter<A, I>(mut self, alloc: &A, iter: &mut I) -> Result<DataPageBuf, A::Error>
    where
        A: PageAlloc,
        I: RewindableIter,
        I::Key: Encodable,
        I::Value: Encodable,
    {
        iter.rewind();
        while let Some((key, value)) = iter.next() {
            self.add(key, value);
        }
        let ptr = self.base.build(alloc, self.size());
        ptr.map(|ptr| unsafe {
            let mut buf = DataPageBuf::new(ptr, self);
            iter.rewind();
            while let Some((key, value)) = iter.next() {
                buf.add(key, value);
            }
            buf
        })
    }
}

pub struct DataPageBuf {
    ptr: PagePtr,
    offsets: *mut u32,
    content: BufWriter,
    current: usize,
}

impl DataPageBuf {
    unsafe fn new(mut ptr: PagePtr, builder: DataPageBuilder) -> Self {
        let offsets = ptr.content_mut() as *mut u32;
        let mut content = BufWriter::new(ptr.content_mut());
        content.skip(builder.offsets_size);
        Self {
            ptr,
            offsets,
            content,
            current: 0,
        }
    }

    unsafe fn add<K, V>(&mut self, key: &K, value: &V)
    where
        K: Encodable,
        V: Encodable,
    {
        let offset = self.content.pos() as u32;
        self.offsets.add(self.current).write(offset.to_le());
        self.current += 1;
        key.encode_to(&mut self.content);
        value.encode_to(&mut self.content);
    }

    pub fn as_ptr(&mut self) -> PagePtr {
        self.ptr
    }

    pub fn as_ref<'a, K, V>(&self) -> DataPageRef<'a, K, V>
    where
        K: Decodable + Ord,
        V: Decodable,
    {
        unsafe { DataPageRef::new(self.ptr) }
    }
}

impl Deref for DataPageBuf {
    type Target = PagePtr;

    fn deref(&self) -> &Self::Target {
        &self.ptr
    }
}

impl DerefMut for DataPageBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ptr
    }
}

/// An immutable reference to a data page.
pub struct DataPageRef<'a, K, V> {
    base: PagePtr,
    offsets: &'a [u32],
    _mark: PhantomData<(K, V)>,
}

impl<'a, K, V> DataPageRef<'a, K, V>
where
    K: Decodable + Ord,
    V: Decodable,
{
    pub unsafe fn new(base: PagePtr) -> Self {
        let offsets_ptr = base.content() as *const u32;
        let offsets_len = if base.content_size() == 0 {
            0
        } else {
            let offset = offsets_ptr.read();
            offset as usize / size_of::<u32>()
        };
        let offsets = slice::from_raw_parts(offsets_ptr, offsets_len);
        Self {
            base,
            offsets,
            _mark: PhantomData,
        }
    }

    /// Returns the number of entries in the page.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Returns the entry at the give position.
    pub fn get(&self, index: usize) -> Option<(K, V)> {
        if let Some(&offset) = self.offsets.get(index) {
            unsafe {
                let ptr = self.content_at(offset);
                let mut buf = BufReader::new(ptr);
                let key = K::decode_from(&mut buf);
                let value = V::decode_from(&mut buf);
                Some((key, value))
            }
        } else {
            None
        }
    }

    /// Returns the first entry that is no less than `target`.
    pub fn seek(&self, target: &K) -> Option<(K, V)> {
        self.get(self.rank(target))
    }

    /// Returns the first entry that is no greater than `target`.
    pub fn seek_back(&self, target: &K) -> Option<(K, V)> {
        let index = self.rank(target);
        for i in (0..=index).rev() {
            if let Some((key, value)) = self.get(i) {
                if &key <= target {
                    return Some((key, value));
                }
            }
        }
        None
    }

    /// Returns an iterator over the entries in the page.
    pub fn iter(&self) -> DataPageIter<'a, K, V> {
        DataPageIter::new(self.clone())
    }

    fn rank(&self, target: &K) -> usize {
        let mut left = 0;
        let mut right = self.len();
        while left < right {
            let mid = (left + right) / 2;
            let key = unsafe {
                let ptr = self.content_at(self.offsets[mid]);
                let mut buf = BufReader::new(ptr);
                K::decode_from(&mut buf)
            };
            match key.cmp(target) {
                Ordering::Less => left = mid + 1,
                Ordering::Greater => right = mid,
                Ordering::Equal => return mid,
            }
        }
        left
    }

    fn content_at(&self, offset: u32) -> *const u8 {
        let offset = offset.to_le() as usize;
        unsafe { self.base.content().add(offset) }
    }
}

impl<'a, K, V> Clone for DataPageRef<'a, K, V> {
    fn clone(&self) -> Self {
        Self {
            base: self.base,
            offsets: self.offsets,
            _mark: PhantomData,
        }
    }
}

impl<'a, K, V> Deref for DataPageRef<'a, K, V> {
    type Target = PagePtr;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

pub struct DataPageIter<'a, K, V> {
    page: DataPageRef<'a, K, V>,
    next: usize,
    last: Option<(K, V)>,
}

impl<'a, K, V> DataPageIter<'a, K, V>
where
    K: Decodable,
    V: Decodable,
{
    pub fn new(page: DataPageRef<'a, K, V>) -> Self {
        Self {
            page,
            next: 0,
            last: None,
        }
    }
}

impl<'a, K, V> ForwardIter for DataPageIter<'a, K, V>
where
    K: Decodable + Ord,
    V: Decodable,
{
    type Key = K;
    type Value = V;

    fn last(&self) -> Option<&(K, V)> {
        self.last.as_ref()
    }

    fn next(&mut self) -> Option<&(K, V)> {
        self.last = self.page.get(self.next).map(|next| {
            self.next += 1;
            next
        });
        self.last.as_ref()
    }
}

impl<'a, K, V> SeekableIter for DataPageIter<'a, K, V>
where
    K: Decodable + Ord,
    V: Decodable,
{
    fn seek(&mut self, target: &K) {
        self.next = self.page.rank(target);
        self.last = None;
    }
}

impl<'a, K, V> RewindableIter for DataPageIter<'a, K, V>
where
    K: Decodable + Ord,
    V: Decodable,
{
    fn rewind(&mut self) {
        self.next = 0;
        self.last = None;
    }
}

#[cfg(test)]
mod test {
    use super::{base::test::ALLOC, *};

    #[test]
    fn data_page() {
        let data = [(1, 0), (2, 0), (4, 0), (7, 0), (8, 0)];
        let mut iter = SliceIter::from(&data);
        let page = DataPageBuilder::default()
            .build_from_iter(&ALLOC, &mut iter)
            .unwrap();

        let page = page.as_ref::<u64, u64>();
        assert_eq!(page.kind(), PageKind::Data);
        assert_eq!(page.is_index(), false);

        assert_eq!(page.seek(&0), Some((1, 0)));
        assert_eq!(page.seek_back(&0), None);
        assert_eq!(page.seek(&3), Some((4, 0)));
        assert_eq!(page.seek_back(&3), Some((2, 0)));
        assert_eq!(page.seek(&9), None);
        assert_eq!(page.seek_back(&9), Some((8, 0)));

        let mut iter = page.iter();
        assert_eq!(iter.last(), None);
        for _ in 0..2 {
            for item in data.iter() {
                assert_eq!(iter.next(), Some(item));
                assert_eq!(iter.last(), Some(item));
            }
            iter.rewind();
        }
    }
}
