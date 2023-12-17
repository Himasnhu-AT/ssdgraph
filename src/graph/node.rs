use serde::{Deserialize, Serialize};

use super::edge::EdgeId;
use super::property::{Properties, Value};

pub type NodeId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub props: Properties,
    /// Head of the outgoing edge chain (None if no outgoing edges yet).
    pub out_head: Option<EdgeId>,
    /// Head of the incoming edge chain (None if no incoming edges yet).
    pub in_head: Option<EdgeId>,
}

impl Node {
    pub fn new(id: NodeId) -> Self {
        Self {
            id,
            labels: Vec::new(),
            props: Properties::default(),
            out_head: None,
            in_head: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }

    pub fn set(mut self, key: impl Into<String>, value: Value) -> Self {
        self.props.insert(key.into(), value);
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("bincode encode never fails for Node")
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
    fn persist_and_reload_node() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("graph.db");

        let node = Node::new(42)
            .with_label("Person")
            .set("name", Value::Str("Alice".into()))
            .set("age", Value::Int(30));

        let (page_id, slot) = {
            let mut pager = Pager::open(&path).unwrap();
            let mut page = pager.alloc_page(PageKind::Node).unwrap();
            let pid = page.id;
            let s = page.push_slot(&node.encode()).unwrap();
            pager.write_page(&page).unwrap();
            pager.sync().unwrap();
            (pid, s)
        };

        let mut pager = Pager::open(&path).unwrap();
        let page = pager.read_page(page_id).unwrap();
        let bytes = page.get_slot(slot).unwrap();
        let loaded = Node::decode(bytes).unwrap();
        assert_eq!(loaded, node);
    }

    #[test]
    fn encodes_all_value_kinds() {
        let n = Node::new(1)
            .with_label("Thing")
            .set("a", Value::Null)
            .set("b", Value::Bool(true))
            .set("c", Value::Int(-7))
            .set("d", Value::Float(3.5))
            .set("e", Value::Str("hi".into()));
        let bytes = n.encode();
        let back = Node::decode(&bytes).unwrap();
        assert_eq!(back, n);
    }
}
