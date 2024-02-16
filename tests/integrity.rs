//! End-to-end integrity checks: page CRC32, WAL-based crash recovery, and
//! single-writer / many-readers behaviour under a Mutex.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use ssdgraph::graph::property::Properties;
use ssdgraph::graph::{Graph, Value};
use ssdgraph::storage::page::{Page, PageKind, PAGE_SIZE};
use ssdgraph::storage::pager::Pager;
use ssdgraph::storage::wal::Wal;

fn p(pairs: &[(&str, Value)]) -> Properties {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[test]
fn wal_replays_after_simulated_crash() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("graph.db");
    let wal_path = dir.path().join("graph.wal");

    let page_id = {
        let mut pager = Pager::open(&data).unwrap();
        let id = pager.alloc_page(PageKind::Node).unwrap().id;
        pager.sync().unwrap();
        id
    };

    {
        let mut wal = Wal::open(&wal_path).unwrap();
        let mut page = Page::new(page_id, PageKind::Node);
        page.push_slot(b"after-crash").unwrap();
        page.seal();
        wal.append_page(page_id, page.as_bytes()).unwrap();
        wal.sync().unwrap();
    }

    let mut pager = Pager::open(&data).unwrap();
    let mut wal = Wal::open(&wal_path).unwrap();
    let n = wal
        .replay(|_id, bytes| {
            let mut page = Page::decode(bytes).expect("WAL bytes carry a valid page");
            pager.write_page(&mut page)
        })
        .unwrap();
    assert_eq!(n, 1);
    pager.sync().unwrap();
    wal.checkpoint().unwrap();

    let page = pager.read_page(page_id).unwrap();
    assert_eq!(page.get_slot(0), Some(&b"after-crash"[..]));
}

#[test]
fn flipped_byte_in_data_file_is_caught() {
    let dir = tempfile::tempdir().unwrap();
    let node_id = {
        let mut g = Graph::open(dir.path()).unwrap();
        let id = g
            .create_node(vec!["X".into()], p(&[("name", Value::Str("orig".into()))]))
            .unwrap();
        g.sync().unwrap();
        id
    };

    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join("graph.db"))
            .unwrap();
        // Page 1 is the first data page. Flip a byte well inside its slot area.
        let target = (PAGE_SIZE + 100) as u64;
        let mut b = [0u8; 1];
        f.seek(SeekFrom::Start(target)).unwrap();
        f.read_exact(&mut b).unwrap();
        f.seek(SeekFrom::Start(target)).unwrap();
        f.write_all(&[b[0] ^ 0xff]).unwrap();
        f.sync_all().unwrap();
    }

    let mut g = Graph::open(dir.path()).unwrap();
    assert!(
        g.get_node(node_id).is_err(),
        "corrupted page should fail CRC verification"
    );
}

#[test]
fn single_writer_many_readers_under_mutex() {
    let dir = tempfile::tempdir().unwrap();
    let g = Arc::new(Mutex::new(Graph::open(dir.path()).unwrap()));

    let writer = {
        let g = Arc::clone(&g);
        thread::spawn(move || {
            for i in 0..100 {
                let mut g = g.lock().unwrap();
                g.create_node(vec!["X".into()], p(&[("i", Value::Int(i))]))
                    .unwrap();
            }
        })
    };

    let readers: Vec<_> = (0..3)
        .map(|_| {
            let g = Arc::clone(&g);
            thread::spawn(move || {
                for _ in 0..200 {
                    let g = g.lock().unwrap();
                    let _labels = g.labels();
                    let _ids = g.nodes_with_label("X");
                }
            })
        })
        .collect();

    writer.join().unwrap();
    for r in readers {
        r.join().unwrap();
    }

    let mut g = g.lock().unwrap();
    g.sync().unwrap();
    assert_eq!(g.nodes_with_label("X").len(), 100);
}
