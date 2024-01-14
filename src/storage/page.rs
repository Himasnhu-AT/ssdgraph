//! 4 KiB fixed-size page. The page layout is:
//!
//! ```text
//!   +------------------------+ 0
//!   | header (16 bytes)      |
//!   +------------------------+ 16
//!   | data slabs grow -->    |
//!   |                        |
//!   |   <-- slot directory   |
//!   +------------------------+ PAGE_SIZE - 4
//!   | CRC32 of bytes[0..end] |
//!   +------------------------+ PAGE_SIZE
//! ```
//!
//! Header: page_id u64 | kind u8 | reserved [3] | slot_count u16 | free_offset u16.
//! The last 4 bytes hold a CRC32 of everything that precedes them. `seal` must
//! be called before the page is written to disk; `decode` verifies the CRC on
//! every read.

pub const PAGE_SIZE: usize = 4096;
pub const HEADER_SIZE: usize = 16;
pub const CRC_SIZE: usize = 4;
pub const SLOT_AREA_END: usize = PAGE_SIZE - CRC_SIZE;

pub type PageId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    FreeList,
    Node,
    Edge,
    Adjacency,
    Property,
}

impl PageKind {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::FreeList => 0,
            Self::Node => 1,
            Self::Edge => 2,
            Self::Adjacency => 3,
            Self::Property => 4,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::FreeList,
            1 => Self::Node,
            2 => Self::Edge,
            3 => Self::Adjacency,
            4 => Self::Property,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Page {
    pub id: PageId,
    pub kind: PageKind,
    pub bytes: Box<[u8; PAGE_SIZE]>,
}

impl Page {
    pub fn new(id: PageId, kind: PageKind) -> Self {
        let mut p = Page {
            id,
            kind,
            bytes: Box::new([0u8; PAGE_SIZE]),
        };
        p.bytes[0..8].copy_from_slice(&id.to_le_bytes());
        p.bytes[8] = kind.to_u8();
        p.set_free_offset(HEADER_SIZE as u16);
        p
    }

    pub fn slot_count(&self) -> u16 {
        u16::from_le_bytes([self.bytes[12], self.bytes[13]])
    }

    fn set_slot_count(&mut self, n: u16) {
        let b = n.to_le_bytes();
        self.bytes[12] = b[0];
        self.bytes[13] = b[1];
    }

    pub fn free_offset(&self) -> u16 {
        u16::from_le_bytes([self.bytes[14], self.bytes[15]])
    }

    fn set_free_offset(&mut self, off: u16) {
        let b = off.to_le_bytes();
        self.bytes[14] = b[0];
        self.bytes[15] = b[1];
    }

    pub fn push_slot(&mut self, data: &[u8]) -> Option<u16> {
        let slot_count = self.slot_count() as usize;
        let free_off = self.free_offset() as usize;
        let slot_dir_end = SLOT_AREA_END - slot_count * 4;
        if free_off + data.len() + 4 > slot_dir_end {
            return None;
        }
        self.bytes[free_off..free_off + data.len()].copy_from_slice(data);
        let slot_pos = slot_dir_end - 4;
        let off_bytes = (free_off as u16).to_le_bytes();
        let len_bytes = (data.len() as u16).to_le_bytes();
        self.bytes[slot_pos] = off_bytes[0];
        self.bytes[slot_pos + 1] = off_bytes[1];
        self.bytes[slot_pos + 2] = len_bytes[0];
        self.bytes[slot_pos + 3] = len_bytes[1];
        self.set_slot_count((slot_count + 1) as u16);
        self.set_free_offset((free_off + data.len()) as u16);
        Some(slot_count as u16)
    }

    pub fn get_slot(&self, idx: u16) -> Option<&[u8]> {
        if idx >= self.slot_count() {
            return None;
        }
        let slot_pos = SLOT_AREA_END - (idx as usize + 1) * 4;
        let off = u16::from_le_bytes([self.bytes[slot_pos], self.bytes[slot_pos + 1]]) as usize;
        let len = u16::from_le_bytes([self.bytes[slot_pos + 2], self.bytes[slot_pos + 3]]) as usize;
        Some(&self.bytes[off..off + len])
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..]
    }

    /// Compute and write the page CRC. Idempotent. Must be called before any
    /// `as_bytes()` consumer that will persist the result.
    pub fn seal(&mut self) {
        let crc = compute_crc(&self.bytes[..SLOT_AREA_END]);
        self.bytes[SLOT_AREA_END..].copy_from_slice(&crc.to_le_bytes());
    }

    pub fn stored_checksum(&self) -> u32 {
        u32::from_le_bytes(self.bytes[SLOT_AREA_END..].try_into().unwrap())
    }

    pub fn verify_checksum(&self) -> bool {
        let stored = self.stored_checksum();
        let computed = compute_crc(&self.bytes[..SLOT_AREA_END]);
        stored == computed
    }

    pub fn decode(bytes: [u8; PAGE_SIZE]) -> Option<Self> {
        let stored = u32::from_le_bytes(bytes[SLOT_AREA_END..].try_into().ok()?);
        let computed = compute_crc(&bytes[..SLOT_AREA_END]);
        if stored != computed {
            return None;
        }
        let id = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let kind = PageKind::from_u8(bytes[8])?;
        Some(Page {
            id,
            kind,
            bytes: Box::new(bytes),
        })
    }
}

fn compute_crc(bytes: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(bytes);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_slots() {
        let mut p = Page::new(7, PageKind::Node);
        let a = p.push_slot(b"alpha").unwrap();
        let b = p.push_slot(b"bravo!!").unwrap();
        let c = p.push_slot(b"charlie").unwrap();
        p.seal();

        let mut arr = [0u8; PAGE_SIZE];
        arr.copy_from_slice(p.as_bytes());
        let q = Page::decode(arr).unwrap();

        assert_eq!(q.id, 7);
        assert_eq!(q.kind, PageKind::Node);
        assert_eq!(q.slot_count(), 3);
        assert_eq!(q.get_slot(a), Some(&b"alpha"[..]));
        assert_eq!(q.get_slot(b), Some(&b"bravo!!"[..]));
        assert_eq!(q.get_slot(c), Some(&b"charlie"[..]));
        assert_eq!(q.get_slot(99), None);
    }

    #[test]
    fn rejects_overflow() {
        let mut p = Page::new(0, PageKind::Node);
        let big = vec![0xAB; PAGE_SIZE];
        assert!(p.push_slot(&big).is_none());
    }

    #[test]
    fn corrupted_page_fails_decode() {
        let mut p = Page::new(3, PageKind::Node);
        p.push_slot(b"hello").unwrap();
        p.seal();

        let mut arr = [0u8; PAGE_SIZE];
        arr.copy_from_slice(p.as_bytes());
        // Flip a byte in the data area.
        arr[100] ^= 0xff;
        assert!(Page::decode(arr).is_none());
    }
}
