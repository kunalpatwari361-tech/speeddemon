# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

## Approach

Rust, standard library only. The file is split into one byte range per hardware thread, and each thread streams its range through a 1 MB buffer, so the input is never held in memory whole. A line belongs to the range that holds its first byte.

Each line is parsed in place without decoding: the hour comes from the timestamp's digits with integer calendar arithmetic instead of a date parser, the latency and status are read backwards from the end of the line, and the endpoint is the bytes in between. Each thread maps endpoints to small ids and keeps one latency list and error count per (endpoint, hour), in a flat array, since the span is fixed at 72 hours.

The threads' results are merged under endpoints sorted by byte, so walking hours and then endpoints produces the output in the required order. Each group's latencies are sorted once to read the three nearest-rank percentiles.

## Dependencies

None.

## With another 24 hours

Replace the per-group latency lists with one radix-sorted array to cut memory, read with larger aligned blocks, and sort groups in parallel.
