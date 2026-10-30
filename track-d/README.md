# Track D: Traverse

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/d-traverse.md).

## Approach

Rust, standard library only. Both input files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

The graph is parsed once on every hardware thread into compact edge triples, counting out-degrees and marking which nodes have an in-edge as it goes. The triples are then placed into a compressed adjacency array, with an atomic cursor per node. Each edge is packed as head and weight in one 64-bit word.

Node ids are a random relabelling, so a search would jump all over memory. Before querying, the nodes are renumbered: one breadth-first search per thread grows a region from its own root, claiming unclaimed nodes, and each region gets a contiguous id range. Neighbours end up with nearby ids, and a search stays in cache.

Queries run in parallel, one per thread at a time, each a Dijkstra search that stops when the target is settled. The heap holds distance and node packed in one 64-bit key. Each thread keeps 32-bit distances, which halves its table; the few searches that pass 2^32 near the edges of weight about 2^32 are redone, one at a time, over a single shared 64-bit table. A target with no in-edges is answered `-1` without searching, which is what would otherwise make a search settle the whole graph.

## Dependencies

None.

## With another 24 hours

Bidirectional search on a reversed graph, and a radix heap in place of the binary heap.
