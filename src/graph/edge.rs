use serde::{Deserialize, Serialize};

use super::node::NodeId;
use super::property::{Properties, Value};

pub type EdgeId = u64;

/// A directed labelled edge. Each edge sits on two singly-linked lists:
///
/// * `next_out` — the next edge in `src`'s outgoing chain
/// * `next_in`  — the next edge in `dst`'s incoming chain
///
/// `Graph` walks these chains during traversal so that listing a node's
/// neighbours costs O(degree) page hops rather than scanning every edge page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub src: NodeId,
    pub dst: NodeId,
    pub type_name: String,
    pub props: Properties,
    pub next_out: Option<EdgeId>,
    pub next_in: Option<EdgeId>,
}

impl Edge {
    pub fn new(id: EdgeId, src: NodeId, dst: NodeId, type_name: impl Into<String>) -> Self {
        Self {
            id,
            src,
            dst,
            type_name: type_name.into(),
            props: Properties::default(),
            next_out: None,
            next_in: None,
        }
    }

    pub fn set(mut self, key: impl Into<String>, value: Value) -> Self {
        self.props.insert(key.into(), value);
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("bincode encode never fails for Edge")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::PageKind;
    use crate::storage::pager::Pager;
    use tempfile::tempdir;

    #[test]
    fn persist_and_reload_edge() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("graph.db");

        let edge = Edge::new(11, 1, 2, "KNOWS")
            .set("since", Value::Int(2019))
            .set("strength", Value::Float(0.8));

        let (page_id, slot) = {
            let mut pager = Pager::open(&path).unwrap();
            let mut page = pager.alloc_page(PageKind::Edge).unwrap();
            let pid = page.id;
            let s = page.push_slot(&edge.encode()).unwrap();
            pager.write_page(&page).unwrap();
            pager.sync().unwrap();
            (pid, s)
        };

        let mut pager = Pager::open(&path).unwrap();
        let page = pager.read_page(page_id).unwrap();
        let bytes = page.get_slot(slot).unwrap();
        let loaded = Edge::decode(bytes).unwrap();
        assert_eq!(loaded, edge);
    }

    #[test]
    fn adjacency_chain_roundtrip() {
        let a = Edge {
            id: 1,
            src: 10,
            dst: 20,
            type_name: "LINKS".into(),
            props: Default::default(),
            next_out: Some(2),
            next_in: None,
        };
        let bytes = a.encode();
        let back = Edge::decode(&bytes).unwrap();
        assert_eq!(back.next_out, Some(2));
        assert_eq!(back.next_in, None);
    }
}
