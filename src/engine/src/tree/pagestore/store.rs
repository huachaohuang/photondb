use crate::tree::{
    page::{PagePtr, PageVer},
    Options, Result,
};

pub struct PageInfo {
    pub ver: PageVer,
    pub len: u8,
    pub is_index: bool,
}

pub struct PageStore {}

#[allow(dead_code)]
impl PageStore {
    pub async fn open(_opts: Options) -> Result<Self> {
        Ok(Self {})
    }

    pub fn page_info(&self, _addr: u64) -> Option<PageInfo> {
        todo!()
    }

    pub async fn load_page(&self, _addr: u64) -> Result<PagePtr> {
        todo!()
    }

    pub fn acquire_page(&self) -> u64 {
        todo!()
    }

    pub fn release_page(&self, _addr: u64) {
        todo!()
    }
}
