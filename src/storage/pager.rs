//! Disk pager: maps `PageId`s to 4 KiB regions of a single file.
//!
//! File layout:
//!   page 0       — header (magic, version, next_page_id, free_head)
//!   page 1..N    — data pages (kind = Node/Edge/Adjacency/Property/FreeList)
//!
//! Allocation pops from the free-list chain when non-empty, otherwise it
//! extends the file by handing out `next_page_id` and bumping the counter.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use super::page::{Page, PageId, PageKind, PAGE_SIZE};
use crate::cache::{PageCache, DEFAULT_CAPACITY};

const MAGIC: &[u8; 8] = b"SSDGRAPH";
const VERSION: u32 = 1;
const HEADER_PAGE: PageId = 0;

pub struct Pager {
    file: File,
    next_page_id: PageId,
    free_head: PageId,
    cache: PageCache,
}

impl Pager {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::open_with_cache(path, DEFAULT_CAPACITY)
    }

    pub fn open_with_cache<P: AsRef<Path>>(path: P, cache_pages: usize) -> io::Result<Self> {
        let path = path.as_ref();
        let exists = path.exists() && std::fs::metadata(path)?.len() >= PAGE_SIZE as u64;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        if exists {
            Self::load_header(file, cache_pages)
        } else {
            Self::init_new(file, cache_pages)
        }
    }

    fn init_new(mut file: File, cache_pages: usize) -> io::Result<Self> {
        let mut header = [0u8; PAGE_SIZE];
        header[0..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..20].copy_from_slice(&1u64.to_le_bytes());
        header[20..28].copy_from_slice(&0u64.to_le_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.sync_data()?;
        Ok(Pager {
            file,
            next_page_id: 1,
            free_head: 0,
            cache: PageCache::new(cache_pages),
        })
    }

    fn load_header(mut file: File, cache_pages: usize) -> io::Result<Self> {
        let mut header = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header)?;
        if &header[0..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad magic"));
        }
        let version = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported version {version}"),
            ));
        }
        let next_page_id = u64::from_le_bytes(header[12..20].try_into().unwrap());
        let free_head = u64::from_le_bytes(header[20..28].try_into().unwrap());
        Ok(Pager {
            file,
            next_page_id,
            free_head,
            cache: PageCache::new(cache_pages),
        })
    }

    fn write_header(&mut self) -> io::Result<()> {
        let mut header = [0u8; PAGE_SIZE];
        header[0..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..20].copy_from_slice(&self.next_page_id.to_le_bytes());
        header[20..28].copy_from_slice(&self.free_head.to_le_bytes());
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        Ok(())
    }

    pub fn alloc_page(&mut self, kind: PageKind) -> io::Result<Page> {
        let id = if self.free_head != 0 {
            let head = self.free_head;
            let head_page = self.read_page_raw(head)?;
            let next = head_page
                .get_slot(0)
                .and_then(|s| s.try_into().ok())
                .map(u64::from_le_bytes)
                .unwrap_or(0);
            self.free_head = next;
            self.cache.invalidate(head);
            head
        } else {
            let id = self.next_page_id;
            self.next_page_id += 1;
            id
        };
        let mut page = Page::new(id, kind);
        self.write_page(&mut page)?;
        self.write_header()?;
        Ok(page)
    }

    pub fn read_page(&mut self, id: PageId) -> io::Result<Arc<Page>> {
        if id == HEADER_PAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "page 0 is header"));
        }
        if let Some(p) = self.cache.get(id) {
            return Ok(p);
        }
        let page = self.read_page_raw(id)?;
        let arc = Arc::new(page);
        self.cache.put(id, Arc::clone(&arc));
        Ok(arc)
    }

    fn read_page_raw(&mut self, id: PageId) -> io::Result<Page> {
        let mut buf = [0u8; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(id * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut buf)?;
        Page::decode(buf).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad page"))
    }

    pub fn write_page(&mut self, page: &mut Page) -> io::Result<()> {
        if page.id == HEADER_PAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot write to header",
            ));
        }
        page.seal();
        self.file.seek(SeekFrom::Start(page.id * PAGE_SIZE as u64))?;
        self.file.write_all(page.as_bytes())?;
        self.cache.invalidate(page.id);
        Ok(())
    }

    pub fn free_page(&mut self, id: PageId) -> io::Result<()> {
        let mut free_page = Page::new(id, PageKind::FreeList);
        free_page
            .push_slot(&self.free_head.to_le_bytes())
            .expect("free-list pointer always fits");
        self.write_page(&mut free_page)?;
        self.free_head = id;
        self.write_header()?;
        Ok(())
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    pub fn next_page_id(&self) -> PageId {
        self.next_page_id
    }

    pub fn cache_stats(&self) -> (u64, u64) {
        (self.cache.hits(), self.cache.misses())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_write_reopen_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("graph.db");

        let id;
        {
            let mut pager = Pager::open(&path).unwrap();
            let mut page = pager.alloc_page(PageKind::Node).unwrap();
            id = page.id;
            page.push_slot(b"hello-world").unwrap();
            pager.write_page(&mut page).unwrap();
            pager.sync().unwrap();
        }

        let mut pager = Pager::open(&path).unwrap();
        let page = pager.read_page(id).unwrap();
        assert_eq!(page.kind, PageKind::Node);
        assert_eq!(page.get_slot(0), Some(&b"hello-world"[..]));
    }

    #[test]
    fn free_list_reuses_pages() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("graph.db");
        let mut pager = Pager::open(&path).unwrap();

        let a = pager.alloc_page(PageKind::Node).unwrap().id;
        let b = pager.alloc_page(PageKind::Node).unwrap().id;
        assert_ne!(a, b);

        pager.free_page(a).unwrap();
        let c = pager.alloc_page(PageKind::Node).unwrap().id;
        assert_eq!(c, a, "freed page should be re-handed out");

        pager.free_page(b).unwrap();
        pager.free_page(c).unwrap();
        let d = pager.alloc_page(PageKind::Node).unwrap().id;
        let e = pager.alloc_page(PageKind::Node).unwrap().id;
        assert!([a, b].contains(&d) && [a, b].contains(&e) && d != e);
    }
}
