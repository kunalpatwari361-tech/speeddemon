# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

## Approach

The scanner is C++ (`native/scan.cpp`); the rest is Rust with its standard library only. The log is memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

The file is split into one part per hardware thread, at line boundaries. Each line is parsed in place without decoding. The hour comes from the timestamp's digits with integer calendar arithmetic, and is only recomputed when the 8 bytes "MM-DDTHH", read as one word, differ from the line before. The method and the endpoint are read 8 bytes at a time: a word's first space shows up as its first zero byte after an xor with spaces, and each word is mixed into the endpoint's hash. The latency is read straight from its digits, and a status only needs its first digit to tell a 5xx.

Endpoints are interned into one lock-free open-addressing table shared by all threads. A slot holds where the endpoint first occurs in the file, so no string is ever copied. A group is (endpoint slot, hour), and the span is fixed at 72 hours.

The file is scanned once. Each line becomes one 4-byte record, appended to the chunk list of its hour in its part: its endpoint's slot, a bit for 5xx, and its latency, or for a latency of 2^18 µs or more (about one line in twenty) its index in the part's overflow list. A chunk is one 4 KB page of 1,020 records, the next page of its part's own arena, so a part's records for an hour are never copied or reallocated, and no allocator is involved.

The same threads that scan then take the hours, once every part is scanned, and one of them sorts the endpoints by byte meanwhile. Hours are processed in parallel, in any order, since the log is not in time order. For an hour, one pass over its records counts each endpoint's requests and 5xx responses, and a second pass places each latency in its endpoint's run of one buffer that stays in cache. The nearest-rank percentiles are selected, not sorted: p50 in the whole run, then p95 among the values above it, and p99 among those above p95. The hours' lines are written in hour order as they finish, and an hour's chunk pages go back to the system as soon as its latencies are placed, adjacent pages in one call, so the output written to `/out` reuses that memory.

The record arena refuses transparent huge pages: one 2 MB page would hold many hours' chunks, and giving back a finished hour's 4 KB pages would not lower the memory counted.

The binary is built for the scoring CPU (`target-cpu=native`).

## Dependencies

None.

## With another 24 hours

The spec promises no bound on how far a line can be out of order, so an hour is complete only when the whole file is scanned, and every record is held at that point: 12.4 MB, beside the 10.1 MB output that `/out` holds in memory. A lower peak needs smaller records, not earlier hours. The hour phase's time is mostly selecting and formatting; a faster selection by buckets and a table of digit pairs were both measured without gain (see PERF.md).
