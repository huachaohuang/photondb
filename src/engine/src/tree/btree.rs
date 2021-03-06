use super::{
    page::*,
    pagecache::{PageAddr, PageCache, PageView},
    pagestore::PageStore,
    pagetable::PageTable,
    Error, Ghost, Options, Result,
};

const ROOT_ID: u64 = 0;
const ROOT_INDEX: Index = Index::with_id(ROOT_ID);

struct Node {
    id: u64,
    view: PageView,
}

type NodeIter<'a, K, V> = MergingIter<DataPageIter<'a, K, V>>;

pub struct BTree {
    opts: Options,
    table: PageTable,
    cache: PageCache,
    store: PageStore,
}

impl BTree {
    pub async fn open(opts: Options) -> Result<Self> {
        let table = PageTable::default();
        let cache = PageCache::default();
        let store = PageStore::open(opts.clone()).await?;
        let tree = Self {
            opts,
            table,
            cache,
            store,
        };
        tree.init()
    }

    pub async fn get<'a, 'g>(
        &'a self,
        key: &[u8],
        lsn: u64,
        ghost: &'g Ghost,
    ) -> Result<Option<&'g [u8]>> {
        let key = Key::new(key, lsn);
        loop {
            match self.try_get(key, ghost).await {
                Err(Error::Again) => continue,
                other => return other,
            }
        }
    }

    async fn try_get<'a, 'g>(&'a self, key: Key<'_>, ghost: &'g Ghost) -> Result<Option<&'g [u8]>> {
        let node = self.try_find_node(key.raw, ghost).await?;
        self.lookup_value(key, &node, ghost).await
    }

    pub async fn put<'g>(
        &self,
        key: &[u8],
        lsn: u64,
        value: &[u8],
        ghost: &'g Ghost,
    ) -> Result<()> {
        let key = Key::new(key, lsn);
        let value = Value::Put(value);
        self.update(key, value, ghost).await
    }

    pub async fn delete<'g>(&self, key: &[u8], lsn: u64, ghost: &'g Ghost) -> Result<()> {
        let key = Key::new(key, lsn);
        let value = Value::Delete;
        self.update(key, value, ghost).await
    }

    async fn update<'g>(&self, key: Key<'_>, value: Value<'_>, ghost: &'g Ghost) -> Result<()> {
        let mut iter = OptionIter::from((key, value));
        let mut page = DataPageBuilder::default().build_from_iter(&self.cache, &mut iter)?;
        loop {
            match self.try_update(key.raw, page.as_ptr(), ghost).await {
                Ok(_) => return Ok(()),
                Err(Error::Again) => continue,
                Err(err) => {
                    unsafe {
                        self.cache.dealloc(page.as_ptr());
                    }
                    return Err(err);
                }
            }
        }
    }

    async fn try_update<'g>(&self, key: &[u8], mut delta: PagePtr, ghost: &'g Ghost) -> Result<()> {
        let mut node = self.try_find_node(key, ghost).await?;
        loop {
            delta.set_ver(node.view.ver());
            delta.set_len(node.view.len() + 1);
            delta.set_next(node.view.as_addr().into());
            match self.table.cas(node.id, delta.next(), delta.into()) {
                Ok(_) => {
                    if delta.len() >= self.opts.data_delta_length {
                        node.view = delta.into();
                        let _ = self.try_consolidate_node::<Key, Value>(&node, ghost).await;
                    }
                    return Ok(());
                }
                Err(addr) => {
                    if let Some(view) = self.page_view(addr.into()) {
                        if view.ver() == node.view.ver() {
                            node.view = view;
                            continue;
                        }
                    }
                    return Err(Error::Again);
                }
            }
        }
    }
}

impl BTree {
    fn init(self) -> Result<Self> {
        let ghost = Ghost::pin();
        // Initializes the tree as root -> leaf.
        let root_id = self.table.alloc(ghost.guard()).unwrap();
        let leaf_id = self.table.alloc(ghost.guard()).unwrap();
        let mut leaf_page = DataPageBuilder::default().build(&self.cache)?;
        self.table.set(leaf_id, leaf_page.as_ptr().into());
        let mut root_iter = OptionIter::from(([].as_slice(), Index::with_id(leaf_id)));
        let mut root_page =
            DataPageBuilder::default().build_from_iter(&self.cache, &mut root_iter)?;
        root_page.set_index(true);
        self.table.set(root_id, root_page.as_ptr().into());
        Ok(self)
    }

    fn node(&self, id: u64) -> Node {
        let addr = self.page_addr(id);
        // Our access pattern ensures that the address must be valid.
        let view = self.page_view(addr).unwrap();
        Node { id, view }
    }

    fn page_addr(&self, id: u64) -> PageAddr {
        self.table.get(id).into()
    }

    fn page_view(&self, addr: PageAddr) -> Option<PageView> {
        match addr {
            PageAddr::Mem(addr) => {
                let page = unsafe { PagePtr::new(addr as *mut u8) };
                page.map(PageView::from)
            }
            PageAddr::Disk(addr) => self
                .store
                .page_info(addr)
                .map(|info| PageView::Disk(info, addr)),
        }
    }

    fn dealloc_page_chain<'g>(&self, mut addr: PageAddr, ghost: &'g Ghost) {
        let cache = self.cache.clone();
        ghost.guard().defer(move || unsafe {
            while let PageAddr::Mem(ptr) = addr {
                if let Some(page) = PagePtr::new(ptr as *mut u8) {
                    addr = page.next().into();
                    cache.dealloc(page);
                } else {
                    break;
                }
            }
        });
    }

    async fn load_page_with_view(&self, id: u64, view: &PageView) -> Result<PagePtr> {
        match *view {
            PageView::Mem(page) => Ok(page),
            PageView::Disk(_, addr) => {
                // self.swapin_page(id, addr).await,
                todo!()
            }
        }
    }

    async fn load_page_with_addr(&self, id: u64, addr: PageAddr) -> Result<Option<PagePtr>> {
        match addr {
            PageAddr::Mem(addr) => {
                let page = unsafe { PagePtr::new(addr as *mut u8) };
                Ok(page)
            }
            PageAddr::Disk(addr) => {
                // self.swapin_page(id, addr).await,
                todo!()
            }
        }
    }

    async fn walk_node<F>(&self, node: &Node, mut f: F) -> Result<()>
    where
        F: FnMut(PagePtr) -> bool,
    {
        let mut page = self.load_page_with_view(node.id, &node.view).await?;
        loop {
            if f(page) {
                break;
            }
            let next = page.next().into();
            match self.load_page_with_addr(node.id, next).await? {
                Some(next) => page = next,
                None => break,
            }
        }
        Ok(())
    }

    async fn iter_node<'g, K, V>(&self, node: &Node, ghost: &'g Ghost) -> Result<NodeIter<'g, K, V>>
    where
        K: Decodable + Ord,
        V: Decodable,
    {
        let mut merger = MergingIterBuilder::default();
        self.walk_node(node, |page| {
            let page = unsafe { TypedPageRef::cast(page) };
            if let TypedPageRef::Data(data) = page {
                merger.add(data.iter());
            }
            false
        })
        .await?;
        Ok(merger.build())
    }

    async fn lookup_value<'a, 'g>(
        &'a self,
        key: Key<'_>,
        node: &Node,
        ghost: &'g Ghost,
    ) -> Result<Option<&'g [u8]>> {
        let mut value = None;
        self.walk_node(node, |page| {
            let page = unsafe { TypedPageRef::<'g, Key, Value>::cast(page) };
            if let TypedPageRef::Data(data) = page {
                if let Some((k, v)) = data.seek(&key) {
                    if k.raw == key.raw {
                        value = v.into();
                        return true;
                    }
                }
            }
            false
        })
        .await?;
        Ok(value)
    }

    async fn lookup_index<'a, 'g>(
        &'a self,
        key: &[u8],
        node: &Node,
        ghost: &'g Ghost,
    ) -> Result<Option<Index>> {
        let mut index = None;
        self.walk_node(node, |page| {
            let page = unsafe { TypedPageRef::<'g, &[u8], Index>::cast(page) };
            if let TypedPageRef::Data(data) = page {
                if let Some((_, v)) = data.seek_back(&key) {
                    index = v.into();
                    return true;
                }
            }
            false
        })
        .await?;
        Ok(index)
    }

    async fn try_find_node<'g>(&self, key: &[u8], ghost: &'g Ghost) -> Result<Node> {
        let mut cursor = ROOT_INDEX;
        let mut parent = None;
        loop {
            let node = self.node(cursor.id);
            if node.view.ver() != cursor.ver {
                self.try_reconcile_node(&node, parent.as_ref(), ghost)?;
                return Err(Error::Again);
            }
            if node.view.is_index() {
                cursor = self.lookup_index(key, &node, ghost).await?.unwrap();
                parent = Some(node);
            } else {
                return Ok(node);
            }
        }
    }

    fn try_reconcile_node<'g>(
        &self,
        node: &Node,
        parent: Option<&Node>,
        ghost: &'g Ghost,
    ) -> Result<()> {
        todo!()
    }

    async fn try_consolidate_node<'g, K, V>(
        &self,
        node: &Node,
        ghost: &'g Ghost,
    ) -> Result<DataPageRef<'g, K, V>>
    where
        K: Encodable + Decodable + Ord,
        V: Encodable + Decodable,
    {
        let mut iter = self.iter_node::<K, V>(node, ghost).await?;
        let mut page = DataPageBuilder::default().build_from_iter(&self.cache, &mut iter)?;
        page.set_ver(node.view.ver());
        page.set_index(node.view.is_index());

        let new_ptr = page.as_ptr();
        let old_addr = node.view.as_addr();
        self.table
            .cas(node.id, old_addr.into(), new_ptr.into())
            .map_err(|_| {
                unsafe { self.cache.dealloc(new_ptr) };
                Error::Again
            })?;

        self.dealloc_page_chain(old_addr, ghost);
        Ok(page.as_ref())
    }
}
