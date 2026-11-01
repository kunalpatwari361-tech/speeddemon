# Track D: Traverse

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/d-traverse.md).

## Approach

Rust, standard library only. Both input files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

The queries are read first. The graph file is then read twice, on every hardware thread; /data is in the page cache, so a read costs time but no memory. The first read counts out-degrees and marks which query targets have an in-edge. The second places every edge into a compressed adjacency, packed as head and weight in one 64-bit word, with an atomic cursor per node.

Node ids are a random relabelling, so a search would jump all over memory. Before querying, the nodes are renumbered: one breadth-first search per thread grows a region from its own root, claiming unclaimed nodes, and each region gets a contiguous id range. Neighbours end up with nearby ids, and a search stays in cache. Each region then copies its nodes' edges, renamed, into its own stretch of the final adjacency, with no third read and no atomics.

Queries are sorted by renumbered source and cut into one block per thread, so a thread's searches stay in one part of the graph and touch only that part of its lazily zeroed distance table; a thread that finishes its block helps with the others. Each query is a Dijkstra search that stops when the target is settled. The heap is a radix heap over distances, which never fall below the last one popped, so each entry is one 64-bit word holding its distance and node. Each thread keeps the heap's buckets between queries. Each thread keeps 16-bit distances (0 meaning unreached), a quarter of a 64-bit table; a search whose target lies beyond that range is redone, one at a time, over a shared 64-bit table. A target with no in-edges is answered `-1` without searching, which avoids settling the whole graph.

The allocator gives every block of 64 KB or more back when it is freed (`mallopt`), so the first adjacency and each search's tables do not stay with the process after they are dropped. A search lists the nodes it reaches, to zero them again, but only up to a sixteenth of the table; past that it clears the whole table. The queries are renamed before the searches, so the old-to-new numbering is freed first.

The binary is built for the scoring CPU (`target-cpu=native`).

## Dependencies

None.

## Next experiment

Profile graph parsing and adjacency construction separately, then look for an improvement that does not increase the working set or the source-line cost.
