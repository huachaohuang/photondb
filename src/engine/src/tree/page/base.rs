use std::{alloc::Layout, ptr::NonNull};

// Page header: ver (6B) | len (1B) | tag (1B) | next (8B) | content_size (4B) |
const PAGE_ALIGNMENT: usize = 8;
const PAGE_HEADER_SIZE: usize = 20;
const PAGE_VERSION_SIZE: usize = 6;

/// A non-null pointer to a page.
#[derive(Copy, Clone, Debug)]
pub struct PagePtr(NonNull<u8>);

impl PagePtr {
    /// Creates a new `PagePtr` if `ptr` is non-null.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it does not check if `ptr` points to a valid page.
    pub unsafe fn new(ptr: *mut u8) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    /// Returns the underlying raw pointer.
    pub const fn as_raw(self) -> *mut u8 {
        self.0.as_ptr()
    }

    unsafe fn ver_ptr(self) -> *mut u8 {
        self.as_raw()
    }

    unsafe fn len_ptr(self) -> *mut u8 {
        self.as_raw().add(PAGE_VERSION_SIZE)
    }

    unsafe fn tag_ptr(self) -> *mut u8 {
        self.as_raw().add(PAGE_VERSION_SIZE + 1)
    }

    unsafe fn next_ptr(self) -> *mut u64 {
        (self.as_raw() as *mut u64).add(1)
    }

    unsafe fn content_size_ptr(self) -> *mut u32 {
        (self.as_raw() as *mut u32).add(4)
    }

    /// Returns the page version.
    pub fn ver(&self) -> PageVer {
        unsafe {
            let mut ver = 0u64;
            let ver_ptr = &mut ver as *mut u64 as *mut u8;
            ver_ptr.copy_from_nonoverlapping(self.ver_ptr(), PAGE_VERSION_SIZE);
            PageVer(u64::from_le(ver))
        }
    }

    pub fn set_ver(&mut self, ver: PageVer) {
        unsafe {
            let ver = ver.0.to_le();
            let ver_ptr = &ver as *const u64 as *const u8;
            ver_ptr.copy_to_nonoverlapping(self.ver_ptr(), PAGE_VERSION_SIZE);
        }
    }

    /// Returns the length of the chain.
    pub fn len(&self) -> u8 {
        unsafe { self.len_ptr().read() }
    }

    pub fn set_len(&mut self, len: u8) {
        unsafe {
            self.len_ptr().write(len);
        }
    }

    /// Returns the address of the next page in the chain.
    pub fn next(&self) -> u64 {
        unsafe { self.next_ptr().read().to_le() }
    }

    pub fn set_next(&mut self, next: u64) {
        unsafe {
            self.next_ptr().write(next.to_le());
        }
    }

    fn tag(&self) -> PageTag {
        unsafe { self.tag_ptr().read().into() }
    }

    fn set_tag(&mut self, tag: PageTag) {
        unsafe {
            self.tag_ptr().write(tag.into());
        }
    }

    /// Returns the page kind.
    pub fn kind(&self) -> PageKind {
        self.tag().kind()
    }

    pub fn set_kind(&mut self, kind: PageKind) {
        self.set_tag(self.tag().with_kind(kind));
    }

    /// Returns true if this is an index page.
    pub fn is_index(&self) -> bool {
        self.tag().is_index()
    }

    pub fn set_index(&mut self, is_index: bool) {
        self.set_tag(self.tag().with_index(is_index));
    }

    /// Sets the page header as default.
    pub fn set_default(&mut self) {
        unsafe { self.as_raw().write_bytes(0, PAGE_HEADER_SIZE) };
    }

    /// Returns a pointer to the page content.
    pub fn content(&self) -> *const u8 {
        unsafe { self.as_raw().add(PAGE_HEADER_SIZE) }
    }

    /// Returns a mutable pointer to the page content.
    pub fn content_mut(&mut self) -> *mut u8 {
        unsafe { self.as_raw().add(PAGE_HEADER_SIZE) }
    }

    /// Returns the page size in bytes.
    pub fn size(&self) -> usize {
        PAGE_HEADER_SIZE + self.content_size() as usize
    }

    /// Returns the page content size in bytes.
    pub fn content_size(&self) -> u32 {
        unsafe { self.content_size_ptr().read().to_le() }
    }

    fn set_content_size(&mut self, size: u32) {
        unsafe {
            self.content_size_ptr().write(size.to_le());
        }
    }
}

impl From<PagePtr> for u64 {
    fn from(ptr: PagePtr) -> Self {
        ptr.as_raw() as u64
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct PageVer(u64);

const PAGE_VERSION_MAX: u64 = (1 << 48) - 1;

impl PageVer {
    pub const fn new(ver: u64) -> Self {
        assert!(ver <= PAGE_VERSION_MAX);
        Self(ver)
    }

    pub const fn next(self) -> Self {
        Self::new(self.0 + 1)
    }
}

impl From<PageVer> for u64 {
    fn from(ver: PageVer) -> Self {
        ver.0
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct PageTag(u8);

const PAGE_KIND_MASK: u8 = 0x7F;

impl PageTag {
    const fn kind(self) -> PageKind {
        PageKind::new(self.0 & PAGE_KIND_MASK)
    }

    const fn with_kind(self, kind: PageKind) -> Self {
        Self(self.index() | kind as u8)
    }

    const fn index(self) -> u8 {
        self.0 & !PAGE_KIND_MASK
    }

    const fn is_index(self) -> bool {
        self.index() != 0
    }

    const fn with_index(self, is_index: bool) -> Self {
        if is_index {
            Self(self.0 | !PAGE_KIND_MASK)
        } else {
            Self(self.0 & PAGE_KIND_MASK)
        }
    }
}

impl From<u8> for PageTag {
    fn from(tag: u8) -> Self {
        Self(tag)
    }
}

impl From<PageTag> for u8 {
    fn from(tag: PageTag) -> Self {
        tag.0
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PageKind {
    Data = 0,
    Split = 1,
}

impl PageKind {
    const fn new(kind: u8) -> Self {
        match kind {
            0 => Self::Data,
            1 => Self::Split,
            _ => panic!("invalid page kind"),
        }
    }
}

impl From<PageKind> for u8 {
    fn from(kind: PageKind) -> Self {
        kind as u8
    }
}

/// An interface to allocate and deallocate pages.
///
/// # Safety
///
/// Similar to the `std::alloc::Allocator` trait.
pub unsafe trait PageAlloc {
    type Error;

    fn alloc(&self, size: usize) -> Result<PagePtr, Self::Error>;

    unsafe fn dealloc(&self, page: PagePtr);

    fn alloc_layout(size: usize) -> Layout {
        unsafe { Layout::from_size_align_unchecked(size, PAGE_ALIGNMENT) }
    }
}

/// A builder to create base pages.
pub struct PageBuilder {
    kind: PageKind,
}

impl PageBuilder {
    pub fn new(kind: PageKind) -> Self {
        Self { kind }
    }

    pub fn build<A>(&self, alloc: &A, content_size: usize) -> Result<PagePtr, A::Error>
    where
        A: PageAlloc,
    {
        assert!(content_size <= u32::MAX as usize);
        let ptr = alloc.alloc(PAGE_HEADER_SIZE + content_size);
        ptr.map(|mut ptr| {
            ptr.set_default();
            ptr.set_kind(self.kind);
            ptr.set_content_size(content_size as u32);
            ptr
        })
    }
}

#[cfg(test)]
pub mod test {
    use std::alloc::GlobalAlloc;

    use jemallocator::{usable_size, Jemalloc};

    use super::*;

    pub const ALLOC: TestAlloc = TestAlloc;

    pub struct TestAlloc;

    unsafe impl PageAlloc for TestAlloc {
        type Error = ();

        fn alloc(&self, size: usize) -> Result<PagePtr, Self::Error> {
            unsafe {
                let ptr = Jemalloc.alloc(Self::alloc_layout(size));
                PagePtr::new(ptr).ok_or(())
            }
        }

        unsafe fn dealloc(&self, page: PagePtr) {
            let ptr = page.as_raw();
            let size = usable_size(ptr);
            Jemalloc.dealloc(ptr, Self::alloc_layout(size));
        }
    }

    #[test]
    fn page_ptr() {
        let mut buf = [1u8; PAGE_HEADER_SIZE];
        let mut ptr = unsafe { PagePtr::new(buf.as_mut_ptr()).unwrap() };
        ptr.set_default();
        assert_eq!(ptr.ver(), PageVer(0));
        ptr.set_ver(ptr.ver().next());
        assert_eq!(ptr.ver(), PageVer(1));
        assert_eq!(ptr.len(), 0);
        ptr.set_len(2);
        assert_eq!(ptr.len(), 2);
        assert_eq!(ptr.next(), 0);
        ptr.set_next(3);
        assert_eq!(ptr.next(), 3);
        assert_eq!(ptr.kind(), PageKind::Data);
        ptr.set_kind(PageKind::Split);
        assert_eq!(ptr.kind(), PageKind::Split);
        assert_eq!(ptr.is_index(), false);
        ptr.set_index(true);
        assert_eq!(ptr.is_index(), true);
        assert_eq!(ptr.content_size(), 0);
        ptr.set_content_size(4);
        assert_eq!(ptr.content_size(), 4);
    }
}
