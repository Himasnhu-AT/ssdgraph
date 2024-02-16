# ssdgraph

A property graph database written in Rust. The shape is Neo4j-like — labelled
nodes, typed directed edges, free-form property maps — but the storage model
is deliberately inverted: every node, edge and property lives on the SSD in
a paged file, and the in-memory cache is intentionally *small* and acts as a
**lookup table**, not as a working-set buffer pool.

The HTTP server exposes a REST API and a dashboard at `/dashboard` that
renders the graph via cytoscape.js, with free-text search and a label filter.

## Architecture

```
   ┌────────────────────────────────────────────────────────────┐
   │   axum server   ─────────────  /api  &  /dashboard         │
   └──────────────┬──────────────────────────┬──────────────────┘
                  │                          │
                  ▼                          ▼
            ┌──────────┐               ┌───────────┐
            │  Graph   │  CRUD  ◄────► │ meta.bin  │  (id → page,slot)
            │ (atomic) │               └───────────┘
            └────┬─────┘
                 │
         ┌───────┴─────────────┐
         │ WAL  ──► Pager      │   write-ahead log fronts every page write
         │                     │
         │  Pager  ──► Cache   │   tiny LRU sits in front of pread/pwrite
         │           │         │
         │           ▼         │
         │   graph.db (4 KiB pages, CRC32-checked) on SSD
         └─────────────────────┘
```

### Why "cache as lookup table"?

Most disk-backed graphs sit a *big* page cache between the user and the
file: queries are fast only when the working set fits in RAM. `ssdgraph`
flips that — the default cache is **256 pages (1 MiB)**. The hot path is
always disk; the cache exists to eliminate repeated reads of the same page
within the same transaction. The interesting consequence is that
**RAM usage is bounded** by the cache size, no matter how big the graph
grows.

### Storage layout

* `graph.db` — paged file, 4 KiB pages, page 0 is the file header (magic,
  version, `next_page_id`, free-list head). Every other page carries a
  type tag (`Node`, `Edge`, `FreeList`), a slot directory and a trailing
  CRC32 covering the rest of the page. Bad pages fail to decode.
* `graph.wal` — append-only write-ahead log. Every page write is appended
  here first; `Graph::sync` then fsyncs the data file and truncates the WAL.
  After a crash, replay reapplies any records the data file lost.
* `meta.bin` — index: id-to-(page, slot) lookups plus the label index.
  Written atomically by `tmp + rename`.

## Quickstart

```sh
cargo run --release -- serve --data ./data --addr 127.0.0.1:7878
```

Then in another shell:

```sh
curl -X POST http://127.0.0.1:7878/api/nodes \
  -H 'content-type: application/json' \
  -d '{"labels":["Person"],"props":{"name":{"Str":"Alice"}}}'

curl -X POST http://127.0.0.1:7878/api/nodes \
  -H 'content-type: application/json' \
  -d '{"labels":["Person"],"props":{"name":{"Str":"Bob"}}}'

curl -X POST http://127.0.0.1:7878/api/edges \
  -H 'content-type: application/json' \
  -d '{"src":1,"dst":2,"type":"KNOWS","props":{}}'
```

Open `http://127.0.0.1:7878/dashboard/` in a browser. The cytoscape canvas
loads the full graph from `/api/graph`; the search box queries
`/api/search?q=…` and the label dropdown filters by label.

## API surface

| Method | Path                          | Notes                                            |
|--------|-------------------------------|--------------------------------------------------|
| GET    | `/api/nodes`                  | All nodes (id, labels, props)                    |
| POST   | `/api/nodes`                  | `{labels, props}` → `{id}`                       |
| GET    | `/api/edges`                  | All edges (id, src, dst, type, props)            |
| POST   | `/api/edges`                  | `{src, dst, type, props}` → `{id}`               |
| GET    | `/api/graph?limit=N`          | Cytoscape-shaped `{nodes, edges}`                |
| GET    | `/api/search?label=&q=&limit=`| Free-text search across labels and string props  |
| GET    | `/dashboard/`                 | The cytoscape dashboard (HTML/CSS/JS)            |

Property values are tagged enums: `{"Str":"Alice"}`, `{"Int":42}`,
`{"Float":3.14}`, `{"Bool":true}`, `"Null"`.

## Embedding

```rust
use ssdgraph::graph::{Graph, Properties, Value, Query};

let mut g = Graph::open("./data")?;
let alice = g.create_node(vec!["Person".into()], Properties::new())?;
let bob   = g.create_node(vec!["Person".into()], Properties::new())?;
let _e    = g.create_edge(alice, bob, "KNOWS", Properties::new())?;
g.sync()?;

let people = g.run_query(&Query::match_label("Person").limit(10))?;
let bfs    = g.bfs(alice, 2)?;
```

## Tests

```sh
cargo test
```

`tests/integrity.rs` covers crash recovery via WAL replay, CRC detection of
a flipped byte in the data file, and single-writer/many-readers under a
mutex.

## Limitations (be honest)

* Single-process, single-writer. There is no cross-process locking on the
  data file. The reasonable use is one `Graph` per process.
* Updated records leak slot space (no online compaction yet).
* The query builder is one node pattern + property equality filters + limit.
  No multi-hop `MATCH` patterns. The traversal API (`bfs` / `dfs`) is the
  escape hatch.
* `meta.bin` is only persisted on `Graph::sync`. A crash without sync loses
  newly-created ids; the underlying page writes survive in the WAL but
  reattaching them to the index requires re-running the create call.
