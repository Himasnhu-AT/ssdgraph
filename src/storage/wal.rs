//! Write-ahead log. Records are appended before the matching page reaches the
//! data file. After a crash, `replay` walks the log and re-applies any record
//! whose page write may have been lost.
//!
//! Record format (little-endian):
//!   u8  record type
//!   u32 payload length
//!   ... payload
//!
//! For [`REC_PAGE_WRITE`] the payload is `page_id (u64) || page_bytes (PAGE_SIZE)`.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::page::{PageId, PAGE_SIZE};

const REC_PAGE_WRITE: u8 = 1;

pub struct Wal {
    path: PathBuf,
    file: std::fs::File,
}

impl Wal {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Wal { path, file })
    }

    pub fn append_page(&mut self, page_id: PageId, bytes: &[u8]) -> io::Result<()> {
        assert_eq!(bytes.len(), PAGE_SIZE, "WAL takes whole pages");
        let len: u32 = (8 + PAGE_SIZE) as u32;
        let mut buf = Vec::with_capacity(5 + 8 + PAGE_SIZE);
        buf.push(REC_PAGE_WRITE);
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&page_id.to_le_bytes());
        buf.extend_from_slice(bytes);
        self.file.write_all(&buf)
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Truncate the log. Call after a checkpoint has fsynced the data file.
    pub fn checkpoint(&mut self) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.sync_all()
    }

    /// Walk all records and apply each page write. Returns the number of
    /// records applied. Torn records at the tail are skipped silently — that
    /// is exactly the crash case we want to tolerate.
    pub fn replay<F>(&mut self, mut apply: F) -> io::Result<usize>
    where
        F: FnMut(PageId, [u8; PAGE_SIZE]) -> io::Result<()>,
    {
        let mut f = OpenOptions::new().read(true).open(&self.path)?;
        let mut count = 0usize;
        loop {
            let mut t = [0u8; 1];
            match f.read_exact(&mut t) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
            let mut len_buf = [0u8; 4];
            if f.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut rec = vec![0u8; len];
            if f.read_exact(&mut rec).is_err() {
                break;
            }
            if t[0] == REC_PAGE_WRITE && rec.len() == 8 + PAGE_SIZE {
                let page_id = u64::from_le_bytes(rec[..8].try_into().unwrap());
                let mut bytes = [0u8; PAGE_SIZE];
                bytes.copy_from_slice(&rec[8..]);
                apply(page_id, bytes)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::{Page, PageKind};
    use crate::storage::pager::Pager;
    use tempfile::tempdir;

    #[test]
    fn replay_applies_lost_writes() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("graph.db");
        let wal_path = dir.path().join("graph.wal");

        let (p1, p2) = {
            let mut pager = Pager::open(&data).unwrap();
            let a = pager.alloc_page(PageKind::Node).unwrap().id;
            let b = pager.alloc_page(PageKind::Node).unwrap().id;
            pager.sync().unwrap();
            (a, b)
        };

        // Simulate writes that only made it to the WAL.
        let mut wal = Wal::open(&wal_path).unwrap();
        let mut a_page = Page::new(p1, PageKind::Node);
        a_page.push_slot(b"one").unwrap();
        wal.append_page(p1, a_page.as_bytes()).unwrap();
        let mut b_page = Page::new(p2, PageKind::Node);
        b_page.push_slot(b"two").unwrap();
        wal.append_page(p2, b_page.as_bytes()).unwrap();
        wal.sync().unwrap();

        // "Crash" then recover: data file has empty pages, WAL has the writes.
        let mut pager = Pager::open(&data).unwrap();
        let mut wal = Wal::open(&wal_path).unwrap();
        let n = wal
            .replay(|_id, bytes| {
                let page = Page::decode(bytes).expect("valid page in WAL");
                pager.write_page(&page)
            })
            .unwrap();
        assert_eq!(n, 2);
        pager.sync().unwrap();
        wal.checkpoint().unwrap();

        assert_eq!(pager.read_page(p1).unwrap().get_slot(0), Some(&b"one"[..]));
        assert_eq!(pager.read_page(p2).unwrap().get_slot(0), Some(&b"two"[..]));
    }

    #[test]
    fn torn_tail_is_ignored() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("torn.wal");
        let mut wal = Wal::open(&wal_path).unwrap();
        let mut p = Page::new(7, PageKind::Node);
        p.push_slot(b"good").unwrap();
        wal.append_page(7, p.as_bytes()).unwrap();
        wal.sync().unwrap();

        // Append garbage that looks like the start of a record but cuts off.
        {
            let mut f = OpenOptions::new().append(true).open(&wal_path).unwrap();
            f.write_all(&[REC_PAGE_WRITE]).unwrap();
            f.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
            f.write_all(b"oops").unwrap();
            f.sync_data().unwrap();
        }

        let mut wal = Wal::open(&wal_path).unwrap();
        let mut applied = 0;
        wal.replay(|_, _| {
            applied += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(applied, 1, "only the well-formed record should replay");
    }
}
