# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

## Approach

Rust, standard library only. The log is memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

The file is split into one part per hardware thread, at line boundaries. Each line is parsed in place without decoding. The hour comes from the timestamp's digits with integer calendar arithmetic, and the date part is only recomputed when it changes from the line before. The endpoint is read 8 bytes at a time: a word's first space shows up as its first zero byte after an xor with spaces, and each word is mixed into the endpoint's hash. The latency and status are read straight from their digits.

Endpoints are interned into one lock-free open-addressing table shared by all threads. A slot holds where the endpoint first occurs in the file, so no string is ever copied. A group is (endpoint slot, hour), and the span is fixed at 72 hours.

The file is parsed once. Each thread counts its requests per group and keeps a compact (group, latency) record per line; 5xx responses go to a shared counter, since they are rare. The counts become, for each thread, its own offset into every group's run of one flat latency array, and each thread then places its records straight into it, with no merging afterwards.

Endpoints are sorted by byte. Hours are split across threads, and each formats its hours' lines in order, sorting each group's run once to read the nearest-rank percentiles. The buffers are written in order.

## Dependencies

None.

## With another 24 hours

Word-at-a-time endpoint scanning, and selecting the three percentiles instead of sorting.
