# Track D: Traverse

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/d-traverse.md).

## Approach

Rust, with `memchr` for newline searches and `memmap2` for read-only file mappings. `/data` is already in the page cache, so reading costs no copy and no memory.

The graph file is read three times, on every hardware thread; /data is in the page cache, so a read costs time but no input copy. The first read counts out-degrees and marks nodes with in-edges. The second places only heads for renumbering. After renumbering, the third pass writes renamed heads and weights directly into the final 64-bit adjacency.

Node ids are a random relabelling, so a search would jump all over memory. Before querying, the nodes are renumbered: one breadth-first search per thread grows a region from its own root, claiming unclaimed nodes, and each region gets a contiguous id range. Neighbours end up with nearby ids, and a search stays in cache.

Queries are sorted by renumbered source and cut into a seven-worker search pool, so each worker touches only its own lazily zeroed distance table; a worker that finishes its block helps with the others. Each query is a Dijkstra search that stops when the target is settled. The heap is a radix heap over distances, which never fall below the last one popped, so each entry is one 64-bit word holding its distance and node. Each worker keeps the heap's buckets between queries. Each worker keeps 16-bit distances (0 meaning unreached); a target beyond that range is retried, one at a time, over a six-byte-per-node packed table (enough for the generator's <2^43 bound). A target with no in-edges is answered `-1` without searching, which avoids settling the whole graph.

The allocator gives every block of 64 KB or more back when it is freed (`mallopt`), so the first adjacency and each search's tables do not stay with the process after they are dropped. A search lists the nodes it reaches, to zero them again, but only up to a sixteenth of the table; past that it clears the whole table. The queries are renamed before the searches, so the old-to-new numbering is freed first.

## Dependencies

`memchr` 2.7.4 and `memmap2` 0.9.5 are vendored and pinned for the offline build.

None.

## Next experiment

Profile graph parsing and adjacency construction separately, then look for an improvement that does not increase the working set or the source-line cost.
