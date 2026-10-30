# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

## Approach

Rust, standard library only. The log is memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

The file is split into one part per hardware thread, at line boundaries. Each line is parsed in place without decoding. The hour comes from the timestamp's digits with integer calendar arithmetic, and the date part is only recomputed when it changes from the line before. The endpoint is hashed while its end is found. The latency and status are read straight from their digits.

Endpoints are interned into one lock-free open-addressing table shared by all threads. A slot holds where the endpoint first occurs in the file, so no string is ever copied. A group is (endpoint slot, hour), and the span is fixed at 72 hours.

Two passes over the file avoid keeping a list per group:

1. Each thread counts its requests per group. 5xx responses go to a shared counter, since they are rare.
2. The counts become, for each thread, its own offset into every group's run of one flat latency array. Each thread then writes its latencies straight into place, with no merging afterwards.

Endpoints are sorted by byte. Hours are split across threads, and each formats its hours' lines in order, sorting each group's run once to read the nearest-rank percentiles. The buffers are written in order.

## Dependencies

None.

## With another 24 hours

Word-at-a-time endpoint scanning, and selecting the three percentiles instead of sorting.
