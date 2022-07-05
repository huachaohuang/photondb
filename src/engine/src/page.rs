use std::{
    collections::BTreeMap,
    mem::size_of_val,
    ops::{
        Bound::{Excluded, Unbounded},
        Deref, DerefMut,
    },
};

use crate::PageId;

#[derive(Copy, Clone, Debug)]
pub struct PageRef<'a>(&'a Page);

impl<'a> PageRef<'a> {
    pub fn from_usize(ptr: usize) -> Option<Self> {
        if ptr == 0 {
            None
        } else {
            Some(unsafe { Self::from_usize_unchecked(ptr) })
        }
    }

    pub unsafe fn from_usize_unchecked(ptr: usize) -> Self {
        Self(&*(ptr as *const Page))
    }

    pub fn into_usize(self) -> usize {
        self.0 as *const Page as usize
    }

    pub fn len(self) -> usize {
        self.0.header.len
    }

    pub fn next(self) -> Option<PageRef<'a>> {
        PageRef::from_usize(self.0.header.next)
    }

    pub fn epoch(self) -> u64 {
        self.0.header.epoch
    }

    pub fn header(self) -> &'a PageHeader {
        &self.0.header
    }

    pub fn is_data(self) -> bool {
        self.0.content.is_data()
    }

    pub fn content(self) -> &'a PageContent {
        &self.0.content
    }
}

#[derive(Debug)]
pub struct PageBuf(Box<Page>);

impl PageBuf {
    pub fn new(header: PageHeader, content: PageContent) -> Self {
        Self(Box::new(Page { header, content }))
    }

    pub fn with_content(content: PageContent) -> Self {
        Self::new(PageHeader::new(), content)
    }

    pub fn with_next<'a>(next: impl Into<PageRef<'a>>, content: PageContent) -> Self {
        Self::new(PageHeader::with_next(next.into()), content)
    }

    pub fn from_usize(ptr: usize) -> Option<Self> {
        if ptr == 0 {
            None
        } else {
            Some(unsafe { Self::from_usize_unchecked(ptr) })
        }
    }

    pub unsafe fn from_usize_unchecked(ptr: usize) -> Self {
        Self(Box::from_raw(ptr as *mut Page))
    }

    pub fn into_usize(self) -> usize {
        Box::into_raw(self.0) as usize
    }

    pub fn as_ref(&self) -> PageRef<'_> {
        PageRef(self.0.as_ref())
    }
}

impl Deref for PageBuf {
    type Target = Page;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PageBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Into<PageRef<'static>> for PageBuf {
    fn into(self) -> PageRef<'static> {
        unsafe { PageRef::from_usize_unchecked(self.into_usize()) }
    }
}

#[derive(Debug)]
pub struct Page {
    header: PageHeader,
    content: PageContent,
}

impl Page {
    pub fn link(&mut self, next: PageRef<'_>) {
        self.header = PageHeader::with_next(next);
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if !self.content.is_removed() {
            PageBuf::from_usize(self.header.next);
        }
    }
}

#[derive(Clone, Debug)]
pub struct PageHeader {
    len: usize,
    next: usize,
    epoch: u64,
}

impl PageHeader {
    fn new() -> Self {
        Self {
            len: 1,
            next: 0,
            epoch: 0,
        }
    }

    fn with_next(next: PageRef<'_>) -> Self {
        let mut header = next.header().clone();
        header.len += 1;
        header.next = next.into_usize();
        header
    }

    pub fn into_next_epoch(mut self) -> Self {
        self.epoch += 1;
        self
    }
}

#[derive(Debug)]
pub enum PageContent {
    BaseData(BaseData),
    DeltaData(DeltaData),
    SplitData(SplitNode),
    MergeData(MergeNode),
    RemoveData,
    BaseIndex(BaseIndex),
    DeltaIndex(DeltaIndex),
    SplitIndex(SplitNode),
    MergeIndex(MergeNode),
    RemoveIndex,
}

impl PageContent {
    pub fn is_data(&self) -> bool {
        match self {
            PageContent::BaseData(_)
            | PageContent::DeltaData(_)
            | PageContent::SplitData(_)
            | PageContent::MergeData(_)
            | PageContent::RemoveData => true,
            _ => false,
        }
    }

    pub fn is_removed(&self) -> bool {
        match self {
            PageContent::RemoveData | PageContent::RemoveIndex => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BaseData {
    size: usize,
    lowest: Vec<u8>,
    highest: Vec<u8>,
    records: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl BaseData {
    pub fn new() -> Self {
        Self {
            size: 0,
            lowest: Vec::new(),
            highest: Vec::new(),
            records: BTreeMap::new(),
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn lowest(&self) -> &[u8] {
        &self.lowest
    }

    pub fn highest(&self) -> &[u8] {
        &self.highest
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.records.get(key).map(|v| v.as_slice())
    }

    pub fn apply(&mut self, delta: DeltaData) {
        for (key, value) in delta.records {
            if let Some(value) = value {
                self.size += key.len() + value.len();
                if let Some(old_value) = self.records.insert(key, value) {
                    self.size -= old_value.len();
                }
            } else {
                if let Some(old_value) = self.records.remove(&key) {
                    self.size -= key.len() + old_value.len();
                }
            }
        }
    }

    pub fn split(&mut self) -> Option<BaseData> {
        let nth = (self.records.len() + 1) / 2;
        if let Some(key) = self.records.keys().nth(nth).cloned() {
            let mut right = BaseData::new();
            right.lowest = key.to_vec();
            right.highest = std::mem::take(&mut self.highest);
            right.records = self.records.split_off(&key);
            right.size = right
                .records
                .iter()
                .fold(0, |acc, (k, v)| acc + k.len() + v.len());
            self.size -= right.size;
            self.highest = key.to_vec();
            Some(right)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeltaData {
    records: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl DeltaData {
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }

    pub fn get(&self, key: &[u8]) -> Option<Option<&[u8]>> {
        self.records
            .get(key)
            .map(|v| v.as_ref().map(|v| v.as_slice()))
    }

    pub fn add(&mut self, key: Vec<u8>, value: Option<Vec<u8>>) {
        self.records.insert(key, value);
    }

    pub fn merge(&mut self, other: DeltaData) {
        for (key, value) in other.records {
            self.records.entry(key).or_insert(value);
        }
    }
}

#[derive(Clone, Debug)]
pub struct PageIndex {
    pub id: PageId,
    pub epoch: u64,
}

#[derive(Clone, Debug)]
pub struct BaseIndex {
    size: usize,
    lowest: Vec<u8>,
    highest: Vec<u8>,
    children: BTreeMap<Vec<u8>, PageIndex>,
}

impl BaseIndex {
    pub fn new() -> Self {
        Self {
            size: 0,
            lowest: Vec::new(),
            highest: Vec::new(),
            children: BTreeMap::new(),
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn lowest(&self) -> &[u8] {
        &self.lowest
    }

    pub fn highest(&self) -> &[u8] {
        &self.highest
    }

    pub fn get(&self, key: &[u8]) -> Option<PageIndex> {
        self.children
            .range(..=key.to_owned())
            .next_back()
            .map(|(_, v)| v.clone())
    }

    pub fn add(&mut self, key: Vec<u8>, index: PageIndex) {
        self.children.insert(key, index);
    }

    pub fn apply(&mut self, deltas: Vec<DeltaIndex>) {
        for delta in deltas.into_iter().rev() {
            // Inserts the new index or merges with the previous one if possible.
            if let Some(index) = self
                .children
                .range_mut(..=delta.lowest.clone())
                .next_back()
                .map(|(_, v)| v)
            {
                if index.id == delta.new_child.id {
                    index.epoch = delta.new_child.epoch;
                } else {
                    self.children.insert(delta.lowest.clone(), delta.new_child);
                }
            } else {
                self.children.insert(delta.lowest.clone(), delta.new_child);
            }
            // Removes range (lowest, highest)
            self.children
                .retain(|k, _| k <= &delta.lowest || k >= &delta.highest);
        }
        self.size = self
            .children
            .iter()
            .fold(0, |acc, (k, v)| acc + k.len() + size_of_val(v));
    }

    pub fn split(&mut self) -> Option<BaseIndex> {
        let nth = (self.children.len() + 1) / 2;
        if let Some(key) = self.children.keys().nth(nth).cloned() {
            let mut right = BaseIndex::new();
            right.lowest = key.to_vec();
            right.highest = std::mem::take(&mut self.highest);
            right.children = self.children.split_off(&key);
            right.size = right
                .children
                .iter()
                .fold(0, |acc, (k, v)| acc + k.len() + size_of_val(v));
            self.size -= right.size;
            self.highest = key.to_vec();
            Some(right)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeltaIndex {
    pub lowest: Vec<u8>,
    pub highest: Vec<u8>,
    pub new_child: PageIndex,
}

impl DeltaIndex {
    pub fn covers(&self, key: &[u8]) -> Option<PageIndex> {
        if key >= &self.lowest && (key < &self.highest || self.highest.is_empty()) {
            Some(self.new_child.clone())
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct SplitNode {
    pub lowest: Vec<u8>,
    pub middle: Vec<u8>,
    pub highest: Vec<u8>,
    pub right_page: PageIndex,
}

#[derive(Debug)]
pub struct MergeNode {
    pub lowest: Vec<u8>,
    pub highest: Vec<u8>,
    pub right_page: PageBuf,
}