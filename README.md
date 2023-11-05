# ssdgraph

A property graph database written in Rust. Inspired by Neo4j, but instead of
keeping the working set in RAM, `ssdgraph` stores every node, edge and property
on disk and keeps only a tiny in-memory cache that acts as a lookup table.

Status: scaffold.
