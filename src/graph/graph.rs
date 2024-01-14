//! High-level graph store. Combines the pager (4 KiB pages on disk), the WAL
//! (durability across crashes) and the page cache (the lookup table) into a
//! single CRUD-shaped API.
//!
//! Metadata — id counters, location of each node/edge, the label index, the
//! currently-writable Node and Edge pages — lives in a small `meta.bin` file
//! alongside the data file. `sync` writes it atomically (`write tmp -> rename`).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::edge::{Edge, EdgeId};
use super::node::{Node, NodeId};
use super::property::Properties;
use crate::storage::page::{Page, PageId, PageKind};
use crate::storage::pager::Pager;
use crate::storage::wal::Wal;

const META_FILE: &str = "meta.bin";
const DATA_FILE: &str = "graph.db";
const WAL_FILE: &str = "graph.wal";

#[derive(Debug, Serialize, Deserialize)]
struct GraphMeta {
    next_node_id: NodeId,
    next_edge_id: EdgeId,
    /// id → (page that contains its bytes, slot index within that page)
    node_loc: HashMap<NodeId, (PageId, u16)>,
    edge_loc: HashMap<EdgeId, (PageId, u16)>,
    /// label → set of node ids carrying that label
    label_index: HashMap<String, HashSet<NodeId>>,
    write_node_page: Option<PageId>,
    write_edge_page: Option<PageId>,
}

impl Default for GraphMeta {
    fn default() -> Self {
        Self {
            next_node_id: 1,
            next_edge_id: 1,
            node_loc: HashMap::new(),
            edge_loc: HashMap::new(),
            label_index: HashMap::new(),
            write_node_page: None,
            write_edge_page: None,
        }
    }
}

pub struct Graph {
    dir: PathBuf,
    pager: Pager,
    wal: Wal,
    meta: GraphMeta,
}

impl Graph {
    pub fn open<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;

        let mut pager = Pager::open(dir.join(DATA_FILE))?;
        let mut wal = Wal::open(dir.join(WAL_FILE))?;

        // Replay any pending WAL records into the data file before we trust
        // the on-disk pages.
        wal.replay(|_id, bytes| {
            let mut p = Page::decode(bytes)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad WAL page"))?;
            pager.write_page(&mut p)
        })?;
        pager.sync()?;
        wal.checkpoint()?;

        let meta = load_meta(&dir)?;

        Ok(Graph {
            dir,
            pager,
            wal,
            meta,
        })
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.wal.sync()?;
        self.pager.sync()?;
        write_meta(&self.dir, &self.meta)?;
        self.wal.checkpoint()?;
        Ok(())
    }

    pub fn cache_stats(&self) -> (u64, u64) {
        self.pager.cache_stats()
    }

    // ----- nodes ----------------------------------------------------------

    pub fn create_node(
        &mut self,
        labels: Vec<String>,
        props: Properties,
    ) -> io::Result<NodeId> {
        let id = self.meta.next_node_id;
        self.meta.next_node_id += 1;

        let node = Node {
            id,
            labels: labels.clone(),
            props,
            out_head: None,
            in_head: None,
        };
        let (pid, slot) = self.append_bytes(PageKind::Node, &node.encode())?;
        self.meta.node_loc.insert(id, (pid, slot));
        for lbl in labels {
            self.meta.label_index.entry(lbl).or_default().insert(id);
        }
        Ok(id)
    }

    pub fn get_node(&mut self, id: NodeId) -> io::Result<Option<Node>> {
        let Some(&(pid, slot)) = self.meta.node_loc.get(&id) else {
            return Ok(None);
        };
        let page = self.pager.read_page(pid)?;
        let Some(bytes) = page.get_slot(slot) else {
            return Ok(None);
        };
        Ok(Node::decode(bytes).ok())
    }

    pub fn update_node(&mut self, node: Node) -> io::Result<()> {
        let id = node.id;
        let old = self
            .get_node(id)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such node"))?;
        let bytes = node.encode();
        let (pid, slot) = self.append_bytes(PageKind::Node, &bytes)?;
        self.meta.node_loc.insert(id, (pid, slot));

        let old_labels: HashSet<&String> = old.labels.iter().collect();
        let new_labels: HashSet<&String> = node.labels.iter().collect();
        for lbl in old_labels.difference(&new_labels) {
            if let Some(s) = self.meta.label_index.get_mut(*lbl) {
                s.remove(&id);
            }
        }
        for lbl in new_labels.difference(&old_labels) {
            self.meta
                .label_index
                .entry((*lbl).clone())
                .or_default()
                .insert(id);
        }
        Ok(())
    }

    pub fn delete_node(&mut self, id: NodeId) -> io::Result<bool> {
        let Some(old) = self.get_node(id)? else {
            return Ok(false);
        };
        self.meta.node_loc.remove(&id);
        for lbl in &old.labels {
            if let Some(s) = self.meta.label_index.get_mut(lbl) {
                s.remove(&id);
            }
        }
        Ok(true)
    }

    pub fn nodes_with_label(&self, label: &str) -> Vec<NodeId> {
        self.meta
            .label_index
            .get(label)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn all_node_ids(&self) -> Vec<NodeId> {
        self.meta.node_loc.keys().copied().collect()
    }

    pub fn labels(&self) -> Vec<String> {
        self.meta.label_index.keys().cloned().collect()
    }

    // ----- edges ----------------------------------------------------------

    pub fn create_edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        type_name: impl Into<String>,
        props: Properties,
    ) -> io::Result<EdgeId> {
        let mut src_node = self.get_node(src)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no node {src}"))
        })?;
        let mut dst_node = if src == dst {
            None
        } else {
            Some(self.get_node(dst)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("no node {dst}"))
            })?)
        };

        let id = self.meta.next_edge_id;
        self.meta.next_edge_id += 1;

        let prev_out = src_node.out_head;
        let prev_in = dst_node.as_ref().map(|n| n.in_head).unwrap_or(src_node.in_head);

        let edge = Edge {
            id,
            src,
            dst,
            type_name: type_name.into(),
            props,
            next_out: prev_out,
            next_in: prev_in,
        };
        let (pid, slot) = self.append_bytes(PageKind::Edge, &edge.encode())?;
        self.meta.edge_loc.insert(id, (pid, slot));

        src_node.out_head = Some(id);
        if let Some(mut n) = dst_node.take() {
            n.in_head = Some(id);
            self.update_node(n)?;
        } else {
            // self-loop
            src_node.in_head = Some(id);
        }
        self.update_node(src_node)?;

        Ok(id)
    }

    pub fn get_edge(&mut self, id: EdgeId) -> io::Result<Option<Edge>> {
        let Some(&(pid, slot)) = self.meta.edge_loc.get(&id) else {
            return Ok(None);
        };
        let page = self.pager.read_page(pid)?;
        let Some(bytes) = page.get_slot(slot) else {
            return Ok(None);
        };
        Ok(Edge::decode(bytes).ok())
    }

    pub fn update_edge(&mut self, edge: Edge) -> io::Result<()> {
        let id = edge.id;
        if !self.meta.edge_loc.contains_key(&id) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such edge"));
        }
        let bytes = edge.encode();
        let (pid, slot) = self.append_bytes(PageKind::Edge, &bytes)?;
        self.meta.edge_loc.insert(id, (pid, slot));
        Ok(())
    }

    pub fn delete_edge(&mut self, id: EdgeId) -> io::Result<bool> {
        Ok(self.meta.edge_loc.remove(&id).is_some())
    }

    pub fn all_edge_ids(&self) -> Vec<EdgeId> {
        self.meta.edge_loc.keys().copied().collect()
    }

    // ----- internals ------------------------------------------------------

    fn append_bytes(&mut self, kind: PageKind, bytes: &[u8]) -> io::Result<(PageId, u16)> {
        let write_page = match kind {
            PageKind::Node => self.meta.write_node_page,
            PageKind::Edge => self.meta.write_edge_page,
            _ => None,
        };
        if let Some(pid) = write_page {
            let page = self.pager.read_page(pid)?;
            let mut owned = (*page).clone();
            drop(page);
            if let Some(slot) = owned.push_slot(bytes) {
                self.write_page_durable(&mut owned)?;
                return Ok((pid, slot));
            }
        }
        let mut new_page = self.pager.alloc_page(kind)?;
        let slot = new_page
            .push_slot(bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "record too large for page"))?;
        self.write_page_durable(&mut new_page)?;
        let pid = new_page.id;
        match kind {
            PageKind::Node => self.meta.write_node_page = Some(pid),
            PageKind::Edge => self.meta.write_edge_page = Some(pid),
            _ => {}
        }
        Ok((pid, slot))
    }

    fn write_page_durable(&mut self, page: &mut Page) -> io::Result<()> {
        page.seal();
        self.wal.append_page(page.id, page.as_bytes())?;
        self.pager.write_page(page)
    }
}

fn load_meta(dir: &Path) -> io::Result<GraphMeta> {
    let p = dir.join(META_FILE);
    if !p.exists() {
        return Ok(GraphMeta::default());
    }
    let bytes = std::fs::read(&p)?;
    bincode::deserialize(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("meta: {e}")))
}

fn write_meta(dir: &Path, meta: &GraphMeta) -> io::Result<()> {
    let tmp = dir.join(format!("{META_FILE}.tmp"));
    let final_path = dir.join(META_FILE);
    let bytes = bincode::serialize(meta)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("meta encode: {e}")))?;
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::property::Value;
    use tempfile::tempdir;

    fn props(pairs: &[(&str, Value)]) -> Properties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn crud_persists_across_reopens() {
        let dir = tempdir().unwrap();

        let (alice, bob, knows) = {
            let mut g = Graph::open(dir.path()).unwrap();
            let alice = g
                .create_node(
                    vec!["Person".into()],
                    props(&[("name", Value::Str("Alice".into()))]),
                )
                .unwrap();
            let bob = g
                .create_node(
                    vec!["Person".into()],
                    props(&[("name", Value::Str("Bob".into()))]),
                )
                .unwrap();
            let knows = g
                .create_edge(alice, bob, "KNOWS", props(&[("since", Value::Int(2019))]))
                .unwrap();
            g.sync().unwrap();
            (alice, bob, knows)
        };

        {
            let mut g = Graph::open(dir.path()).unwrap();
            assert_eq!(
                g.get_node(alice).unwrap().unwrap().props.get("name"),
                Some(&Value::Str("Alice".into()))
            );
            let e = g.get_edge(knows).unwrap().unwrap();
            assert_eq!(e.src, alice);
            assert_eq!(e.dst, bob);
            assert!(g.nodes_with_label("Person").contains(&alice));
            assert!(g.nodes_with_label("Person").contains(&bob));

            assert!(g.delete_node(bob).unwrap());
            g.sync().unwrap();
        }

        let mut g = Graph::open(dir.path()).unwrap();
        assert!(g.get_node(alice).unwrap().is_some());
        assert!(g.get_node(bob).unwrap().is_none());
        let people = g.nodes_with_label("Person");
        assert!(people.contains(&alice));
        assert!(!people.contains(&bob));
    }

    #[test]
    fn update_node_changes_labels_and_props() {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();

        let id = g
            .create_node(vec!["Draft".into()], props(&[("title", Value::Str("x".into()))]))
            .unwrap();
        let mut node = g.get_node(id).unwrap().unwrap();
        node.labels = vec!["Published".into()];
        node.props
            .insert("title".into(), Value::Str("Hello".into()));
        g.update_node(node).unwrap();
        g.sync().unwrap();

        let mut g = Graph::open(dir.path()).unwrap();
        let n = g.get_node(id).unwrap().unwrap();
        assert_eq!(n.labels, vec!["Published".to_string()]);
        assert_eq!(n.props.get("title"), Some(&Value::Str("Hello".into())));
        assert!(g.nodes_with_label("Published").contains(&id));
        assert!(!g.nodes_with_label("Draft").contains(&id));
    }

    #[test]
    fn adjacency_heads_are_updated() {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();
        let a = g.create_node(vec![], Properties::new()).unwrap();
        let b = g.create_node(vec![], Properties::new()).unwrap();
        let e1 = g.create_edge(a, b, "L", Properties::new()).unwrap();
        let e2 = g.create_edge(a, b, "L", Properties::new()).unwrap();

        let a_node = g.get_node(a).unwrap().unwrap();
        assert_eq!(a_node.out_head, Some(e2), "newest edge is the head");
        let e2_edge = g.get_edge(e2).unwrap().unwrap();
        assert_eq!(e2_edge.next_out, Some(e1));
    }
}
