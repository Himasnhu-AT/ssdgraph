//! Tiny pattern-match query builder. Inspired by Cypher's
//! `MATCH (n:Label) WHERE n.k = v RETURN n LIMIT k`, scoped down to one node
//! pattern. The label index narrows the candidate set; the where-filters then
//! short-circuit when `limit` is hit.

use std::io;

use super::graph::Graph;
use super::node::{Node, NodeId};
use super::property::Value;

#[derive(Debug, Default, Clone)]
pub struct Query {
    label: Option<String>,
    filters: Vec<(String, Value)>,
    limit: Option<usize>,
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn match_label(label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            ..Self::default()
        }
    }

    pub fn where_prop(mut self, key: impl Into<String>, value: Value) -> Self {
        self.filters.push((key.into(), value));
        self
    }

    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }
}

impl Graph {
    pub fn run_query(&mut self, q: &Query) -> io::Result<Vec<Node>> {
        let candidates: Vec<NodeId> = match &q.label {
            Some(label) => self.nodes_with_label(label),
            None => self.all_node_ids(),
        };

        let mut out = Vec::new();
        for id in candidates {
            let Some(node) = self.get_node(id)? else {
                continue;
            };
            if q.filters
                .iter()
                .all(|(k, v)| node.props.get(k) == Some(v))
            {
                out.push(node);
                if let Some(lim) = q.limit {
                    if out.len() >= lim {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::property::Properties;
    use tempfile::tempdir;

    fn props(pairs: &[(&str, Value)]) -> Properties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn match_label_then_filter() {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();
        g.create_node(vec!["Person".into()], props(&[("city", Value::Str("Paris".into()))])).unwrap();
        g.create_node(vec!["Person".into()], props(&[("city", Value::Str("Berlin".into()))])).unwrap();
        g.create_node(vec!["Person".into()], props(&[("city", Value::Str("Paris".into()))])).unwrap();
        g.create_node(vec!["Robot".into()], props(&[("city", Value::Str("Paris".into()))])).unwrap();

        let parisians = g
            .run_query(
                &Query::match_label("Person").where_prop("city", Value::Str("Paris".into())),
            )
            .unwrap();
        assert_eq!(parisians.len(), 2);
        assert!(parisians.iter().all(|n| n.labels.contains(&"Person".to_string())));
    }

    #[test]
    fn limit_caps_results() {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();
        for _ in 0..10 {
            g.create_node(vec!["T".into()], Properties::new()).unwrap();
        }
        let res = g.run_query(&Query::match_label("T").limit(3)).unwrap();
        assert_eq!(res.len(), 3);
    }

    #[test]
    fn no_label_scans_everything() {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();
        g.create_node(vec!["A".into()], props(&[("k", Value::Int(1))])).unwrap();
        g.create_node(vec!["B".into()], props(&[("k", Value::Int(1))])).unwrap();
        g.create_node(vec!["B".into()], props(&[("k", Value::Int(2))])).unwrap();
        let res = g.run_query(&Query::new().where_prop("k", Value::Int(1))).unwrap();
        assert_eq!(res.len(), 2);
    }
}
