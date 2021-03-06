use std::{
    alloc::GlobalAlloc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use jemallocator::{usable_size, Jemalloc};

use super::{
    page::{PageAlloc, PagePtr, PageVer},
    pagestore::PageInfo,
    Error, Result,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PageAddr {
    Mem(u64),
    Disk(u64),
}

const MEM_DISK_MASK: u64 = 1 << 63;

impl From<u64> for PageAddr {
    fn from(addr: u64) -> Self {
        if addr & MEM_DISK_MASK == 0 {
            Self::Mem(addr)
        } else {
            Self::Disk(addr & !MEM_DISK_MASK)
        }
    }
}

impl From<PageAddr> for u64 {
    fn from(addr: PageAddr) -> u64 {
        match addr {
            PageAddr::Mem(addr) => addr,
            PageAddr::Disk(addr) => addr | MEM_DISK_MASK,
        }
    }
}

pub enum PageView {
    Mem(PagePtr),
    Disk(PageInfo, u64),
}

impl PageView {
    pub fn ver(&self) -> PageVer {
        match self {
            Self::Mem(page) => page.ver(),
            Self::Disk(info, _) => info.ver,
        }
    }

    pub fn len(&self) -> u8 {
        match self {
            Self::Mem(page) => page.len(),
            Self::Disk(info, _) => info.len,
        }
    }

    pub fn is_index(&self) -> bool {
        match self {
            Self::Mem(page) => page.is_index(),
            Self::Disk(info, _) => info.is_index,
        }
    }

    pub fn as_addr(&self) -> PageAddr {
        match *self {
            Self::Mem(page) => PageAddr::Mem(page.into()),
            Self::Disk(_, addr) => PageAddr::Disk(addr),
        }
    }
}

impl From<PagePtr> for PageView {
    fn from(page: PagePtr) -> Self {
        PageView::Mem(page)
    }
}

#[derive(Clone)]
pub struct PageCache {
    size: Arc<AtomicUsize>,
}

impl Default for PageCache {
    fn default() -> Self {
        Self {
            size: Arc::new(AtomicUsize::new(0)),
        }
    }
}

unsafe impl PageAlloc for PageCache {
    type Error = Error;

    fn alloc(&self, size: usize) -> Result<PagePtr> {
        unsafe {
            let ptr = Jemalloc.alloc(Self::alloc_layout(size));
            let size = usable_size(ptr);
            self.size.fetch_add(size, Ordering::Relaxed);
            PagePtr::new(ptr).ok_or(Error::Alloc)
        }
    }

    unsafe fn dealloc(&self, page: PagePtr) {
        let ptr = page.as_raw();
        let size = usable_size(ptr);
        self.size.fetch_sub(size, Ordering::Relaxed);
        Jemalloc.dealloc(ptr, Self::alloc_layout(size));
    }
}
