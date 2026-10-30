# Track C: Join

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/c-join.md).

## Approach

Rust, standard library only. Both files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

`regions.csv` becomes one byte per region id. A country code `[A-Z]{3}` is used directly as an index into 26³ slots, so the countries come out in byte order without sorting, and each region maps to its country's rank.

`records.csv` is split into one part per hardware thread. A part starts at a true record boundary: an LF followed by a digit or a BOM. Inside a quoted note an LF is always followed by a lowercase word or a quote, so a boundary can't be mistaken. Each thread parses its records in a single pass over the bytes: a BOM is skipped where a record starts, a quoted field ends at the first `"` that isn't a `""` pair, and its value length is its raw length minus its escapes. Count, id sum, cent sums, min, max and byte totals are kept per country per thread and summed at the end.

Distinct names are counted without storing any strings. Every name gets a slot in one lock-free open-addressing table shared by all threads, keyed by a 64-bit hash of the name's raw bytes. The raw bytes identify the value because escaping is one to one, and among 100,000 names a 64-bit collision is about 1 in 4 billion. Each country has a bitset over those slots, and its distinct count is the bitset's population count.

## Dependencies

None.

## With another 24 hours

Word-at-a-time scanning for quotes and commas, and a smaller bitset per country.
