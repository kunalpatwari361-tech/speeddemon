# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

## Approach

The scanner is C++ (`native/scan.cpp`); the rest is Rust with its standard library only. The log is memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

The file is split into one part per hardware thread, at line boundaries. Each line is parsed in place without decoding. The hour comes from the timestamp's digits with integer calendar arithmetic, and is only recomputed when the 8 bytes "MM-DDTHH", read as one word, differ from the line before. The method and the endpoint are read 8 bytes at a time: a word's first space shows up as its first zero byte after an xor with spaces, and each word is mixed into the endpoint's hash. The latency is read straight from its digits, and a status only needs its first digit to tell a 5xx.

Endpoints are interned into one lock-free open-addressing table shared by all threads. A slot holds where the endpoint first occurs in the file, so no string is ever copied. A group is (endpoint slot, hour), and the span is fixed at 72 hours.

The file is scanned once. Each line becomes one 6-byte record, its latency and its endpoint's slot with a bit for 5xx, appended to the chunk list of its hour in its part. A chunk is one 4 KB page of 680 records, the next page of its part's own arena, so a part's records for an hour are never copied or reallocated, and no allocator is involved.

Endpoints are sorted by byte. Hours are then processed in parallel, in any order, since the log is not in time order. For an hour, one pass over its records counts each endpoint's requests and 5xx responses, and a second pass places each latency in its endpoint's run of one buffer that stays in cache. The nearest-rank percentiles are selected, not sorted: p50 in the whole run, then p95 among the values above it, and p99 among those above p95. The hours' lines are written in hour order as they finish, and an hour's chunk pages go back to the system as soon as its latencies are placed, adjacent pages in one call, so the output written to `/out` reuses that memory.

The binary is built for the scoring CPU (`target-cpu=native`).

## Dependencies

None.

## With another 24 hours

Smaller records, since their 6 bytes each are most of the peak memory, and selecting the three percentiles instead of sorting.
