//! Graph traversal — walks the adjacency chains on disk via [`Graph`].
//!
//! `neighbors` is the primitive: it follows the outgoing chain from a node
//! by hopping `Edge::next_out` pointers. `bfs` / `dfs` layer breadth-first
//! and depth-first searches on top, both bounded by a `max_depth` (depth 0
//! returns just the start node).

use std::collections::{HashSet, VecDeque};
use std::io;

use super::edge::EdgeId;
use super::graph::Graph;
use super::node::NodeId;

impl Graph {
    /// Edges leaving `id`, paired with their destination node.
    pub fn neighbors(&mut self, id: NodeId) -> io::Result<Vec<(EdgeId, NodeId)>> {
        let Some(node) = self.get_node(id)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut cur = node.out_head;
        while let Some(eid) = cur {
            let edge = self.get_edge(eid)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, format!("dangling edge {eid}"))
            })?;
            out.push((eid, edge.dst));
            cur = edge.next_out;
        }
        Ok(out)
    }

    /// BFS from `start`, returning node ids in the order they were visited.
    /// `max_depth` is inclusive: `bfs(start, 0)` yields `[start]`.
    pub fn bfs(&mut self, start: NodeId, max_depth: usize) -> io::Result<Vec<NodeId>> {
        let mut visited = HashSet::new();
        let mut order = Vec::new();
        let mut frontier: VecDeque<(NodeId, usize)> = VecDeque::new();

        if self.get_node(start)?.is_none() {
            return Ok(order);
        }
        visited.insert(start);
        frontier.push_back((start, 0));

        while let Some((node, depth)) = frontier.pop_front() {
            order.push(node);
            if depth >= max_depth {
                continue;
            }
            for (_, dst) in self.neighbors(node)? {
                if visited.insert(dst) {
                    frontier.push_back((dst, depth + 1));
                }
            }
        }
        Ok(order)
    }

    /// DFS from `start`, pre-order. Mirrors `bfs`'s `max_depth` semantics.
    pub fn dfs(&mut self, start: NodeId, max_depth: usize) -> io::Result<Vec<NodeId>> {
        let mut visited = HashSet::new();
        let mut order = Vec::new();
        let mut stack: Vec<(NodeId, usize)> = Vec::new();

        if self.get_node(start)?.is_none() {
            return Ok(order);
        }
        stack.push((start, 0));

        while let Some((node, depth)) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            order.push(node);
            if depth >= max_depth {
                continue;
            }
            // Push neighbours in reverse so the first one is popped first —
            // matches the natural left-to-right reading of the adjacency list.
            let mut neighbours = self.neighbors(node)?;
            neighbours.reverse();
            for (_, dst) in neighbours {
                if !visited.contains(&dst) {
                    stack.push((dst, depth + 1));
                }
            }
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::property::Properties;
    use tempfile::tempdir;

    /// Build:
    ///   a -> b -> d
    ///   a -> c
    fn small_graph() -> (tempfile::TempDir, Graph, [NodeId; 4]) {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();
        let a = g.create_node(vec![], Properties::new()).unwrap();
        let b = g.create_node(vec![], Properties::new()).unwrap();
        let c = g.create_node(vec![], Properties::new()).unwrap();
        let d = g.create_node(vec![], Properties::new()).unwrap();
        g.create_edge(a, b, "L", Properties::new()).unwrap();
        g.create_edge(a, c, "L", Properties::new()).unwrap();
        g.create_edge(b, d, "L", Properties::new()).unwrap();
        (dir, g, [a, b, c, d])
    }

    #[test]
    fn neighbors_lists_outgoing_destinations() {
        let (_dir, mut g, [a, b, c, _d]) = small_graph();
        let mut got: Vec<NodeId> = g
            .neighbors(a)
            .unwrap()
            .into_iter()
            .map(|(_, d)| d)
            .collect();
        got.sort();
        let mut want = [b, c];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn bfs_respects_depth_bound() {
        let (_dir, mut g, [a, _, _, d]) = small_graph();
        let depth0 = g.bfs(a, 0).unwrap();
        assert_eq!(depth0, vec![a]);

        let depth1 = g.bfs(a, 1).unwrap();
        assert_eq!(depth1.len(), 3, "root + two neighbours");
        assert!(!depth1.contains(&d));

        let depth2 = g.bfs(a, 2).unwrap();
        assert_eq!(depth2.len(), 4);
        assert!(depth2.contains(&d));
    }

    #[test]
    fn dfs_returns_all_reachable_within_depth() {
        let (_dir, mut g, [a, b, c, d]) = small_graph();
        let order = g.dfs(a, 5).unwrap();
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], a);
        for n in [b, c, d] {
            assert!(order.contains(&n));
        }
    }

    #[test]
    fn missing_start_returns_empty() {
        let dir = tempdir().unwrap();
        let mut g = Graph::open(dir.path()).unwrap();
        assert!(g.bfs(999, 3).unwrap().is_empty());
        assert!(g.dfs(999, 3).unwrap().is_empty());
    }
}
